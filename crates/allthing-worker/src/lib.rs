// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's Cloudflare Worker: a read-only, edge-cached API over Hypersnap
//! and keyless public chain RPCs, built for the Workers Free plan.
//!
//! Static assets (the WebAssembly app, styles, label shards) are served by
//! Workers Static Assets without invoking this code; only `/api/*` runs here.
//! Every page costs one API call, every payload is edge-cached with
//! stale-while-revalidate, and every upstream call goes through a per-isolate
//! rate guard and a per-invocation subrequest budget (see docs/BUDGET.md).

mod cache;
mod host;

use std::cell::Cell;
use std::rc::Rc;

use allthing_core::api::{API_PREFIX, ApiError};
use allthing_data::{Endpoint, Failure, Invocation, upstream};
use futures::future::FutureExt;
use worker::{Context, Env, Headers, Method, Request, Response, Result, Url, event};

use crate::cache::Served;
use crate::host::WorkerHost;

/// The workspace version and the short commit this Worker was built from.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = match option_env!("ALLTHING_COMMIT") {
    Some(commit) => commit,
    None => "dev",
};

/// Cloudflare allows 50 subrequests (fetches and Cache API calls) per
/// invocation on the Free plan; keep a margin for the response path.
const SUBREQUEST_BUDGET: u32 = 45;

thread_local! {
    static STARTED: Cell<f64> = const { Cell::new(0.0) };
    static REQUESTS: Cell<u64> = const { Cell::new(0) };
}

#[event(fetch)]
pub async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    let url = req.url()?;

    // www.allthing.social -> allthing.social (pages redirect client-side).
    if let Some(apex) = url.host_str().and_then(|h| h.strip_prefix("www.")).map(str::to_owned) {
        let mut target = url.clone();
        target.set_host(Some(&apex))?;
        return Response::redirect_with_status(target, 301);
    }
    if url.path() == "/version" || url.path() == "/api/v1/version" {
        return version();
    }
    if !url.path().starts_with("/api/") {
        return error(404, "Not found", None);
    }
    if !matches!(req.method(), Method::Get | Method::Head) {
        return error(405, "Allthing's API is read-only", None);
    }
    if let Some(limited) = rate_limited(&req, &env).await {
        return Ok(limited);
    }

    let inv = Rc::new(Invocation::new(Rc::new(WorkerHost::new(ctx)), SUBREQUEST_BUDGET));
    STARTED.with(|s| {
        if s.get() == 0.0 {
            s.set(inv.now());
        }
    });
    REQUESTS.with(|r| r.set(r.get() + 1));
    route(&inv, &url).await
}

/// Per-client limit from the `API_LIMITER` binding, when configured: a
/// backstop against a single client draining the daily request allowance.
async fn rate_limited(req: &Request, env: &Env) -> Option<Response> {
    let limiter = env.rate_limiter("API_LIMITER").ok()?;
    let client = req.headers().get("cf-connecting-ip").ok().flatten().unwrap_or_else(|| "anonymous".into());
    let outcome = limiter.limit(client).await.ok()?;
    (!outcome.success).then(|| error(429, "Too many requests; slow down", Some(60)).ok()).flatten()
}

fn query(url: &Url, key: &str) -> Option<String> {
    url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

async fn route(inv: &Rc<Invocation>, url: &Url) -> Result<Response> {
    let Some(name) = url.path().strip_prefix(API_PREFIX) else {
        return error(404, "Unknown API version", None);
    };
    if name == "/status" {
        return status(inv);
    }
    let endpoint = match Endpoint::parse(name, |key| query(url, key)) {
        Ok(endpoint) => Rc::new(endpoint),
        Err(bad) => return error(bad.status, bad.message, None),
    };
    let key = format!("{}/__cache{API_PREFIX}/{}", url.origin().ascii_serialization(), endpoint.cache_name());
    serve(inv, key, endpoint).await
}

async fn serve(inv: &Rc<Invocation>, key: String, endpoint: Rc<Endpoint>) -> Result<Response> {
    let build: cache::Builder = Rc::new(move |inv, previous| {
        let endpoint = endpoint.clone();
        async move { endpoint.build(inv, previous).await }.boxed_local()
    });
    match cache::serve(inv, &key, build).await {
        Ok((entry, served)) => {
            let headers = Headers::new();
            headers.set("content-type", "application/json; charset=utf-8")?;
            let browser = if served == Served::StaleIfError { 15 } else { entry.policy.browser };
            headers.set("cache-control", &format!("public, max-age={browser}"))?;
            headers.set("x-allthing-cache", served.label())?;
            let age = ((inv.now() - entry.fetched_at) / 1000.0).max(0.0);
            headers.set("x-allthing-age", &format!("{age:.0}"))?;
            if served == Served::StaleIfError {
                headers.set("x-allthing-stale", "1")?;
            }
            Ok(Response::from_bytes(entry.body.as_ref().clone())?.with_status(entry.status).with_headers(headers))
        }
        Err(Failure { status, message, retry_after }) => error(status, &message, retry_after),
    }
}

fn error(status: u16, message: &str, retry_after: Option<u32>) -> Result<Response> {
    let headers = Headers::new();
    headers.set("content-type", "application/json; charset=utf-8")?;
    headers.set("cache-control", "no-store")?;
    if let Some(seconds) = retry_after {
        headers.set("retry-after", &seconds.to_string())?;
    }
    let body = ApiError { error: message.to_owned(), retry_after };
    Ok(Response::from_json(&body)?.with_status(status).with_headers(headers))
}

/// `{"name","version","commit"}`: which release is deployed.
fn version() -> Result<Response> {
    let headers = Headers::new();
    headers.set("content-type", "application/json; charset=utf-8")?;
    headers.set("cache-control", "no-store")?;
    let body = serde_json::json!({ "name": "allthing", "version": VERSION, "commit": COMMIT });
    Ok(Response::from_json(&body)?.with_headers(headers))
}

/// Health and guard state of the isolate that answers. Makes no upstream
/// calls and touches no cache.
fn status(inv: &Invocation) -> Result<Response> {
    let now = inv.now();
    let body = serde_json::json!({
        "ok": true,
        "isolate": {
            "up_seconds": ((now - STARTED.with(Cell::get)) / 1000.0).round(),
            "requests": REQUESTS.with(Cell::get),
        },
        "upstreams": upstream::guard_status(now),
        "subrequest_budget": SUBREQUEST_BUDGET,
    });
    let headers = Headers::new();
    headers.set("content-type", "application/json; charset=utf-8")?;
    headers.set("cache-control", "no-store")?;
    Ok(Response::from_json(&body)?.with_headers(headers))
}

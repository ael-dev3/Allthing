// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's Cloudflare Worker: a read-only, edge-cached API over Hypersnap
//! and keyless public chain RPCs, built for the Workers Free plan.
//!
//! Static assets (the WebAssembly app, styles, label shards) are served by
//! Workers Static Assets without invoking this code; only `/api/*` runs here.
//! Every page costs one API call, every payload is edge-cached with
//! stale-while-revalidate, and every upstream call goes through a per-isolate
//! rate guard and a per-invocation subrequest budget (see docs/BUDGET.md).

mod annotate;
mod cache;
mod chain;
mod hypersnap;
mod pages;
mod upstream;

use std::cell::Cell;
use std::rc::Rc;

use allthing_core::api::{API_PREFIX, ApiError};
use allthing_core::params::{channel_key_from_param, hash_from_param, username_from_param};
use futures::future::FutureExt;
use worker::{Context, Date, Env, Headers, Method, Request, Response, Result, Url, event};

use crate::cache::{Builder, Failure, Served};

/// Cloudflare allows 50 subrequests (fetches and Cache API calls) per
/// invocation on the Free plan; keep a margin for the response path.
const SUBREQUEST_BUDGET: u32 = 45;

/// Per-request state shared by everything one invocation does, including its
/// background refreshes.
pub struct Invocation {
    ctx: Context,
    budget: Cell<u32>,
}

impl Invocation {
    fn new(ctx: Context) -> Invocation {
        Invocation { ctx, budget: Cell::new(SUBREQUEST_BUDGET) }
    }

    /// Unix milliseconds.
    pub fn now(&self) -> f64 {
        Date::now().as_millis() as f64
    }

    pub fn take_subrequests(&self, n: u32) -> bool {
        let left = self.budget.get();
        if left >= n {
            self.budget.set(left - n);
            true
        } else {
            false
        }
    }

    pub fn remaining_subrequests(&self) -> u32 {
        self.budget.get()
    }

    pub fn wait_until(&self, future: impl std::future::Future<Output = ()> + 'static) {
        self.ctx.wait_until(future);
    }
}

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
    if !url.path().starts_with("/api/") {
        return error(404, "Not found", None);
    }
    if !matches!(req.method(), Method::Get | Method::Head) {
        return error(405, "Allthing's API is read-only", None);
    }
    if let Some(limited) = rate_limited(&req, &env).await {
        return Ok(limited);
    }

    let inv = Rc::new(Invocation::new(ctx));
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
    let Some(endpoint) = url.path().strip_prefix(API_PREFIX) else {
        return error(404, "Unknown API version", None);
    };
    let origin = url.origin().ascii_serialization();
    let key = |name: &str| format!("{origin}/__cache{API_PREFIX}/{name}");

    match endpoint {
        "/home" => serve(inv, key("home"), Rc::new(|inv, prev| pages::home(inv, prev).boxed_local())).await,
        "/channel" => {
            let Some(id) = query(url, "id").and_then(|v| channel_key_from_param(&v)) else {
                return error(400, "Expected ?id=<channel id or parent URL>", None);
            };
            let cache_key = key(&format!("channel?id={}", allthing_core::params::percent_encode(&id)));
            serve(inv, cache_key, Rc::new(move |inv, prev| pages::channel(inv, id.clone(), prev).boxed_local())).await
        }
        "/cast" => {
            let Some(hash) = query(url, "hash").and_then(|v| hash_from_param(&v)) else {
                return error(400, "Expected ?hash=<40 hex characters>", None);
            };
            let cache_key = key(&format!("cast?hash={hash}"));
            serve(inv, cache_key, Rc::new(move |inv, prev| pages::cast(inv, hash.clone(), prev).boxed_local())).await
        }
        "/user" => {
            let Some(name) = query(url, "name").and_then(|v| username_from_param(&v)) else {
                return error(400, "Expected ?name=<username>", None);
            };
            let cache_key = key(&format!("user?name={}", allthing_core::params::percent_encode(&name)));
            serve(inv, cache_key, Rc::new(move |inv, prev| pages::user(inv, name.clone(), prev).boxed_local())).await
        }
        "/wallets" => {
            let Some(fid) = query(url, "fid").and_then(|v| v.parse::<u64>().ok()).filter(|f| *f > 0) else {
                return error(400, "Expected ?fid=<number>", None);
            };
            serve(
                inv,
                key(&format!("wallets?fid={fid}")),
                Rc::new(move |inv, _| pages::wallets(inv, fid).boxed_local()),
            )
            .await
        }
        "/channels" => {
            let q = query(url, "q").map(|q| q.trim().to_lowercase()).filter(|q| !q.is_empty());
            if q.as_ref().is_some_and(|q| q.chars().count() > 64) {
                return error(400, "Search query too long", None);
            }
            let cache_key = match &q {
                Some(q) => key(&format!("channels?q={}", allthing_core::params::percent_encode(q))),
                None => key("channels"),
            };
            serve(inv, cache_key, Rc::new(move |inv, _| pages::channels(inv, q.clone()).boxed_local())).await
        }
        "/status" => status(inv),
        _ => error(404, "Unknown endpoint", None),
    }
}

async fn serve(inv: &Rc<Invocation>, key: String, build: Builder) -> Result<Response> {
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

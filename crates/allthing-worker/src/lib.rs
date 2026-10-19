// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's Cloudflare Worker: a read-only, edge-cached API over Hypersnap
//! and keyless public chain RPCs, built for the Workers Free plan.
//!
//! Static assets (the WebAssembly app, styles, fonts) are served by
//! Workers Static Assets without invoking this code; `/api/*` runs here, and
//! `/wasm/*`, the app's WebAssembly compressed at brotli's highest level
//! (`wasm_module`).
//! Every page costs one API call, every payload is edge-cached with
//! stale-while-revalidate, and every upstream call goes through a per-isolate
//! rate guard and a per-invocation subrequest budget (see docs/BUDGET.md).

mod cache;
mod host;
mod index;

use std::cell::Cell;
use std::rc::Rc;

use allthing_core::api::{API_PREFIX, ApiError};
use allthing_core::index::IndexerState;
use allthing_core::latest;
use allthing_core::ranking::TrendingWindow;
use allthing_core::time::iso_utc;
use allthing_data::{Endpoint, Failure, Invocation, upstream};
use allthing_data::{indexer, pages};
use futures::future::FutureExt;
use worker::{
    Context, EncodeBody, Env, Headers, Method, Request, Response, ResponseBody, ResponseBuilder, Result,
    ScheduleContext, ScheduledEvent, Url, console_error, console_log, event,
};

use crate::cache::Served;
use crate::host::{Lifetime, WorkerHost};
use crate::index::D1Index;

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
    if let Some(name) = url.path().strip_prefix("/wasm/") {
        return wasm_module(&req, &env, &url, name).await;
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

    let index = D1Index::new(&env).map(Rc::new);
    let host = WorkerHost::new(Lifetime::Request(ctx), index, solana_rpc(&env));
    let inv = Rc::new(Invocation::new(Rc::new(host), SUBREQUEST_BUDGET));
    STARTED.with(|s| {
        if s.get() == 0.0 {
            s.set(inv.now());
        }
    });
    REQUESTS.with(|r| r.set(r.get() + 1));
    route(&inv, &url).await
}

/// The cron trigger in wrangler.toml that stores the newest casts. It fires
/// every minute, as the indexer's does, but in an invocation of its own,
/// with its own CPU time and subrequests.
const LATEST_CRON: &str = "*/1 * * * *";

/// The cron triggers in wrangler.toml, every minute: the indexer (one page
/// of casts into Allthing's index, see `allthing_core::index`), and the
/// stored copy of the newest casts (see `pages::store_latest`).
/// The keyed Solana RPC, from the `SOLANA_RPC_URL` secret. Unset, Solana's
/// wallet counts are read from the public endpoint, which usually refuses.
fn solana_rpc(env: &Env) -> Option<String> {
    env.secret("SOLANA_RPC_URL").ok().map(|s| s.to_string()).filter(|u| !u.is_empty())
}

#[event(scheduled)]
pub async fn scheduled(event: ScheduledEvent, env: Env, ctx: ScheduleContext) {
    let Some(index) = D1Index::new(&env).map(Rc::new) else {
        console_error!("cron: no INDEX binding");
        return;
    };
    let host = WorkerHost::new(Lifetime::Schedule(ctx), Some(index.clone()), solana_rpc(&env));
    let inv = Invocation::new(Rc::new(host), SUBREQUEST_BUDGET);
    if event.cron() == LATEST_CRON {
        match pages::store_latest(Rc::new(inv)).await {
            Ok(()) => console_log!("latest: stored"),
            Err(failure) => console_error!("latest: {}", failure.message),
        }
        return;
    }
    match indexer::run(&inv, index.as_ref()).await {
        Ok(report) => console_log!("indexer: {report:?}"),
        Err(error) => console_error!("indexer: {error}"),
    }
}

/// `/wasm/<name>`: the app's WebAssembly. For a browser that accepts brotli,
/// the copy the build compressed at brotli's highest level
/// (`/assets/<name>.br`), about a fifth smaller than the edge compresses the
/// module on the fly; otherwise, or without that copy, the module itself.
/// `boot.js` falls back to `/assets/<name>` if this fails, so the app still
/// loads when the Worker can't answer.
async fn wasm_module(req: &Request, env: &Env, url: &Url, name: &str) -> Result<Response> {
    let ours = name.starts_with("allthing-")
        && name.ends_with(".wasm")
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    if !ours {
        return error(404, "Not found", None);
    }
    let assets = env.assets("ASSETS")?;
    let origin = url.origin().ascii_serialization();
    // A missing file comes back as the app's index.html (single-page
    // routing), which must never be sent as the module.
    let found = |res: &Response| -> Result<bool> {
        let html = res.headers().get("content-type")?.is_some_and(|t| t.starts_with("text/html"));
        Ok(res.status_code() == 200 && !html)
    };
    let brotli = req.headers().get("accept-encoding")?.is_some_and(|accepted| {
        accepted.split(',').any(|coding| coding.split(';').next().map(str::trim) == Some("br"))
    });
    if brotli {
        let compressed = assets.fetch(format!("{origin}/assets/{name}.br"), None).await?;
        if found(&compressed)? {
            let headers = Headers::new();
            headers.set("content-type", "application/wasm")?;
            headers.set("content-encoding", "br")?;
            headers.set("cache-control", "public, max-age=31536000, immutable")?;
            headers.set("vary", "accept-encoding")?;
            let body: ResponseBody = compressed.body().clone();
            return Ok(ResponseBuilder::new().with_headers(headers).with_encode_body(EncodeBody::Manual).body(body));
        }
    }
    let module = assets.fetch(format!("{origin}/assets/{name}"), None).await?;
    if found(&module)? { Ok(module) } else { error(404, "Not found", None) }
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
        return status(inv, url).await;
    }
    let endpoint = match Endpoint::parse(name, |key| query(url, key)) {
        Ok(endpoint) => Rc::new(endpoint),
        Err(bad) => return error(bad.status, bad.message, None),
    };
    let key = cache_key(url, &endpoint.cache_name());
    serve(inv, key, endpoint).await
}

/// Where a payload is cached: its endpoint's cache name on this origin.
fn cache_key(url: &Url, name: &str) -> String {
    format!("{}/__cache{API_PREFIX}/{name}", url.origin().ascii_serialization())
}

async fn serve(inv: &Rc<Invocation>, key: String, endpoint: Rc<Endpoint>) -> Result<Response> {
    let build: cache::Builder = Rc::new(move |inv, previous| {
        let endpoint = endpoint.clone();
        async move { endpoint.build(inv, previous).await }.boxed_local()
    });
    let started = inv.now();
    match cache::serve(inv, &key, build).await {
        Ok((entry, served)) => {
            let headers = Headers::new();
            headers.set("content-type", "application/json; charset=utf-8")?;
            let browser = if served == Served::StaleIfError { 15 } else { entry.policy.browser };
            headers.set("cache-control", &format!("public, max-age={browser}"))?;
            headers.set("x-allthing-cache", served.label())?;
            // The Worker's own time for this answer: a cache read, or the
            // build a miss waited for. Clocks only advance across I/O, so it
            // counts waiting on the cache and upstream, not CPU.
            // Temporary: the build's stages follow the total, as cumulative
            // durations of each stage (see `Invocation::mark`).
            let stages: String =
                inv.stage_durations(started).iter().map(|(name, took)| format!(", {name};dur={took:.0}")).collect();
            headers.set("server-timing", &format!("{};dur={:.0}{stages}", served.label(), inv.now() - started))?;
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

/// Health and guard state of the isolate that answers, and how fresh the
/// cached "new" and trending lists are, and the cron's stored copy of the
/// newest casts. Makes no upstream calls: it reads the two lists from the
/// edge cache and the copy from D1.
async fn status(inv: &Invocation, url: &Url) -> Result<Response> {
    let now = inv.now();
    let stored = async {
        let index = inv.index()?;
        index.snapshot(pages::LATEST_SNAPSHOT).await.ok().flatten()
    };
    let (new, trending, stored) = futures::future::join3(
        cache::peek(inv, &cache_key(url, &Endpoint::Latest.cache_name())),
        cache::peek(inv, &cache_key(url, &Endpoint::Home(0, TrendingWindow::Day).cache_name())),
        stored,
    )
    .await;
    let freshness = |entry: Option<cache::Entry>| entry.and_then(|e| latest::freshness(&e.body, e.fetched_at, now));
    let body = serde_json::json!({
        "freshness": {
            "new": freshness(new),
            "trending": freshness(trending),
            // The cron's copy of "new", which a data center with no recent
            // list of its own serves: null if there is none.
            "new_stored": stored.and_then(|s| latest::freshness(s.body.as_bytes(), s.built, now)),
        },
        "index": index_status(inv).await,
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

/// The index's rankings and the indexer's progress, or why there are none.
async fn index_status(inv: &Invocation) -> serde_json::Value {
    let Some(index) = inv.index() else { return serde_json::json!({ "available": false }) };
    match index.load().await {
        Ok(saved) => {
            let state = IndexerState::load(saved.state.as_deref());
            let rankings: serde_json::Map<String, serde_json::Value> = saved
                .rankings
                .iter()
                .map(|m| {
                    let ranked = serde_json::json!({
                        "ranked_at": iso_utc(m.built / 1000),
                        "casts": m.total,
                        "floor": m.floor,
                    });
                    (m.span.name().to_owned(), ranked)
                })
                .collect();
            serde_json::json!({
                "available": true,
                "rankings": rankings,
                "runs": state.runs,
                "feeds": state.feeds.len(),
                "finished": state.finished.len(),
                "today": { "rows_read": state.read, "rows_written": state.written },
            })
        }
        Err(error) => serde_json::json!({ "available": false, "error": error.to_string() }),
    }
}

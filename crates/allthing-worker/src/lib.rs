// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's Cloudflare Worker: a read-only, edge-cached API over Hypersnap
//! and keyless public chain RPCs, built for the Workers Free plan; Sign In
//! With Farcaster (`auth`), which says who is reading; and the endorsement
//! of a reader's app key (`app_key`), which their browser keeps and signs
//! their likes and recasts with. The Worker posts nothing of its own and
//! never has a reader's key: it only keeps the casts a reader's browser
//! signed for later (`scheduled_casts`), and gives them to the node at
//! their time; and it keeps the images and videos readers attach to their
//! casts, in R2, and serves them at `/m/` (`media`). After a reader's write,
//! the pages it changes are rebuilt on their next read (`refresh`).
//!
//! Static assets (the WebAssembly app, styles, fonts) are served by
//! Workers Static Assets without invoking this code; `/api/*` runs here,
//! `/wasm/*`, the app's WebAssembly compressed at brotli's highest level
//! (`wasm_module`), and `/m/*`, readers' attachments.
//! Every page costs one API call, every payload is edge-cached with
//! stale-while-revalidate, and every upstream call goes through a per-isolate
//! rate guard and a per-invocation subrequest budget (see docs/BUDGET.md).

mod app_key;
mod attest;
mod auth;
mod cache;
mod host;
mod index;
mod media;
mod poll;
mod refresh;
mod scheduled_casts;

use std::cell::Cell;
use std::rc::Rc;

use allthing_core::api::{API_PREFIX, ApiError, REFRESH_PATH};
use allthing_core::app_key::writes_path;
use allthing_core::compose::gif_key_path;
use allthing_core::following;
use allthing_core::index::IndexerState;
use allthing_core::latest;
use allthing_core::media::{MEDIA_PATH, SERVED_PREFIX};
use allthing_core::notifications::{NOTIFICATIONS_PATH, cursor_from_param};
use allthing_core::params::percent_encode;
use allthing_core::poll::POLL_PATH;
use allthing_core::ranking::TrendingWindow;
use allthing_core::scheduled::SCHEDULED_PATH;
use allthing_core::time::iso_utc;
use allthing_data::{Endpoint, Failure, Invocation, upstream};
use allthing_data::{activity, indexer, pages, scheduled};
use futures::future::FutureExt;
use worker::{
    Context, Date, EncodeBody, Env, Headers, Method, Request, Response, ResponseBody, ResponseBuilder, Result,
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
    // Readers' attachments, to anyone (other apps load them from here too),
    // so not under the API's per-client limit: browsers keep each for a
    // year.
    if url.path().starts_with(SERVED_PREFIX) {
        return media::serve(&req, &env, url.path()).await;
    }
    if !url.path().starts_with("/api/") {
        return error(404, "Not found", None);
    }
    // Signing in is the one exchange that isn't a read (POST, never cached).
    if let Some(name) = url.path().strip_prefix("/api/v1/auth/") {
        if let Some(limited) = rate_limited(&req, &env).await {
            return Ok(limited);
        }
        let host = WorkerHost::new(Lifetime::Request(ctx), None, None, node_stand_in(&env));
        let inv = Rc::new(Invocation::new(Rc::new(host), SUBREQUEST_BUDGET));
        return auth::handle(name, req, &env, inv).await;
    }
    // A signed-in reader's scheduled casts: GET, and POST (never cached).
    if let Some(rest) = url.path().strip_prefix(SCHEDULED_PATH) {
        if let Some(limited) = rate_limited(&req, &env).await {
            return Ok(limited);
        }
        return scheduled_casts::handle(rest, req, &env, (Date::now().as_millis() / 1000) as i64).await;
    }
    // A signed-in reader's write, for the pages it changes to be rebuilt:
    // POST, never cached, nothing upstream.
    if url.path() == REFRESH_PATH {
        if let Some(limited) = rate_limited(&req, &env).await {
            return Ok(limited);
        }
        let host = WorkerHost::new(Lifetime::Request(ctx), None, None, node_stand_in(&env));
        let inv = Invocation::new(Rc::new(host), SUBREQUEST_BUDGET);
        return refresh::handle(req, &env, &url, &inv, (Date::now().as_millis() / 1000) as i64).await;
    }
    // A signed-in reader's attachments: GET, and POST (never cached).
    if url.path() == MEDIA_PATH {
        if let Some(limited) = rate_limited(&req, &env).await {
            return Ok(limited);
        }
        return media::handle(req, &env, &ctx, Date::now().as_millis() as f64).await;
    }
    // A vote in Allthing's poll, signed in or not: POST, never cached, one
    // D1 batch, nothing upstream.
    if url.path() == POLL_PATH {
        if let Some(limited) = rate_limited(&req, &env).await {
            return Ok(limited);
        }
        return poll::handle(req, &env).await;
    }
    if !matches!(req.method(), Method::Get | Method::Head) {
        return error(405, "Allthing's API is read-only", None);
    }
    if let Some(limited) = rate_limited(&req, &env).await {
        return Ok(limited);
    }
    // The GIF picker's key, for a signed-in session: no subrequest.
    if url.path() == gif_key_path() {
        return auth::gif_key(&req, &env, (Date::now().as_millis() / 1000) as i64);
    }
    // Whether a signed-in session can like and recast: no subrequest.
    if url.path() == writes_path() {
        return auth::writes(&req, &env, (Date::now().as_millis() / 1000) as i64);
    }

    let index = D1Index::new(&env).map(Rc::new);
    let host = WorkerHost::new(Lifetime::Request(ctx), index, solana_rpc(&env), node_stand_in(&env));
    let inv = Rc::new(Invocation::new(Rc::new(host), SUBREQUEST_BUDGET));
    STARTED.with(|s| {
        if s.get() == 0.0 {
            s.set(inv.now());
        }
    });
    REQUESTS.with(|r| r.set(r.get() + 1));
    // The signed-in reader's own notifications: whose is the session's.
    if url.path() == NOTIFICATIONS_PATH {
        return notifications(&inv, &req, &env, &url).await;
    }
    // The signed-in reader's front page: the casts of whom the session follows.
    if url.path() == following::FOLLOWING_PATH {
        return following_feed(&inv, &req, &env, &url).await;
    }
    route(&inv, &url, &env).await
}

/// `/api/v1/notifications[?cursor=]`: the signed-in reader's
/// notifications (`pages::notifications`), for the account their session
/// names, never one a query names; 401 without a session. Cached at the
/// edge per account and page like any payload (a minute fresh), and
/// answered `private, no-store`, so no browser or shared cache keeps one
/// for another reader.
async fn notifications(inv: &Rc<Invocation>, req: &Request, env: &Env, url: &Url) -> Result<Response> {
    let Some(fid) = auth::signed_in_fid(req, env, (inv.now() / 1000.0) as i64)? else {
        return error(401, "Sign in to see your notifications", None);
    };
    let cursor = match query(url, "cursor").filter(|value| !value.is_empty()) {
        None => None,
        Some(value) => match cursor_from_param(&value) {
            Some(cursor) => Some(cursor),
            None => return error(400, "Expected ?cursor= as a page of notifications gives it", None),
        },
    };
    let name = match &cursor {
        Some(cursor) => format!("notifications?fid={fid}&cursor={}", percent_encode(cursor)),
        None => format!("notifications?fid={fid}"),
    };
    let build: cache::Builder = Rc::new(move |inv, previous| {
        let cursor = cursor.clone();
        async move { pages::notifications(inv, fid, cursor, previous).await }.boxed_local()
    });
    answer(inv, cache_key(url, &name), build, false, false, true).await
}

/// `/api/v1/following[?cursor=]`: the signed-in reader's front page
/// (`pages::following`), the casts of the accounts their session's account
/// follows, never those a query names; 401 without a session. Cached at the
/// edge per account and page (a minute fresh), so one reader's is never
/// another's, and answered `private, no-store`.
async fn following_feed(inv: &Rc<Invocation>, req: &Request, env: &Env, url: &Url) -> Result<Response> {
    let Some(fid) = auth::signed_in_fid(req, env, (inv.now() / 1000.0) as i64)? else {
        return error(401, "Sign in to see the casts of the people you follow", None);
    };
    let cursor = match query(url, "cursor").filter(|value| !value.is_empty()) {
        None => None,
        Some(value) => match following::cursor_from_param(&value) {
            Some(cursor) => Some(cursor),
            None => return error(400, "Expected ?cursor= as a page of the feed gives it", None),
        },
    };
    let name = match &cursor {
        Some(cursor) => format!("following?fid={fid}&cursor={cursor}"),
        None => format!("following?fid={fid}"),
    };
    let build: cache::Builder = Rc::new(move |inv, previous| {
        let cursor = cursor.clone();
        async move { pages::following(inv, fid, cursor, previous).await }.boxed_local()
    });
    answer(inv, cache_key(url, &name), build, false, false, true).await
}

/// The cron trigger in wrangler.toml that stores the newest casts. It fires
/// every minute, as the indexer's does, but in an invocation of its own,
/// with its own CPU time and subrequests.
const LATEST_CRON: &str = "*/1 * * * *";

/// The cron trigger that counts daily active accounts, every two minutes
/// (see `allthing_data::activity`).
const ACTIVITY_CRON: &str = "*/2 * * * *";

/// The cron triggers in wrangler.toml, every minute: the indexer (one page
/// of casts into Allthing's index, see `allthing_core::index`), after the
/// scheduled casts that are due (`allthing_data::scheduled`); and the
/// stored copy of the newest casts (see `pages::store_latest`).
/// The keyed Solana RPC, from the `SOLANA_RPC_URL` secret. Unset, Solana's
/// wallet counts are read from the public endpoint, which usually refuses.
fn solana_rpc(env: &Env) -> Option<String> {
    env.secret("SOLANA_RPC_URL").ok().map(|s| s.to_string()).filter(|u| !u.is_empty())
}

/// The `HYPERSNAP_ORIGIN` variable, which only the end-to-end tests set: a
/// stand-in for Hypersnap's node on this machine
/// (`message::is_local_stand_in`), so a test sees what the Worker hands the
/// node, a scheduled cast among it, and says what the node answers. Never
/// set where Allthing is deployed; anything but a local port is ignored.
fn node_stand_in(env: &Env) -> Option<String> {
    let origin = env.var("HYPERSNAP_ORIGIN").ok()?.to_string();
    allthing_core::message::is_local_stand_in(&origin).then_some(origin)
}

#[event(scheduled)]
pub async fn scheduled(event: ScheduledEvent, env: Env, ctx: ScheduleContext) {
    let Some(index) = D1Index::new(&env).map(Rc::new) else {
        console_error!("cron: no INDEX binding");
        return;
    };
    let host = WorkerHost::new(Lifetime::Schedule(ctx), Some(index.clone()), solana_rpc(&env), node_stand_in(&env));
    let inv = Invocation::new(Rc::new(host), SUBREQUEST_BUDGET);
    if event.cron() == LATEST_CRON {
        match pages::store_latest(Rc::new(inv)).await {
            Ok(()) => console_log!("latest: stored"),
            Err(failure) => console_error!("latest: {}", failure.message),
        }
        return;
    }
    if event.cron() == ACTIVITY_CRON {
        match activity::run(&inv, index.as_ref()).await {
            Ok(report) => console_log!("activity: {report:?}"),
            Err(error) => console_error!("activity: {error}"),
        }
        return;
    }
    // Scheduled casts first: they are due on the minute.
    match scheduled::run(&inv, index.as_ref(), (inv.now() / 1000.0) as i64).await {
        Ok(report) if report.due > 0 => console_log!("scheduled: {report:?}"),
        Ok(_) => {}
        Err(error) => console_error!("scheduled: {error}"),
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

async fn route(inv: &Rc<Invocation>, url: &Url, env: &Env) -> Result<Response> {
    let Some(name) = url.path().strip_prefix(API_PREFIX) else {
        return error(404, "Unknown API version", None);
    };
    if name == "/status" {
        return status(inv, url, env).await;
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
    let wallets = matches!(endpoint.as_ref(), Endpoint::Wallets(_));
    let engagement = matches!(endpoint.as_ref(), Endpoint::Engagement(_));
    let build: cache::Builder = Rc::new(move |inv, previous| {
        let endpoint = endpoint.clone();
        async move { endpoint.build(inv, previous).await }.boxed_local()
    });
    answer(inv, key, build, wallets, engagement, false).await
}

/// A payload from the edge cache, built with `build` when it must be. A
/// `private` one (a session's own) is never kept by a browser or a shared
/// cache.
async fn answer(
    inv: &Rc<Invocation>,
    key: String,
    build: cache::Builder,
    wallets: bool,
    engagement: bool,
    private: bool,
) -> Result<Response> {
    let started = inv.now();
    match cache::serve(inv, &key, build).await {
        Ok((entry, served)) => {
            let headers = Headers::new();
            headers.set("content-type", "application/json; charset=utf-8")?;
            let browser = if served == Served::StaleIfError { 15 } else { entry.policy.browser };
            if private {
                headers.set("cache-control", "private, no-store")?;
            } else {
                headers.set("cache-control", &format!("public, max-age={browser}"))?;
            }
            headers.set("x-allthing-cache", served.label())?;
            // The Worker's own time for this answer: a cache read, or the
            // build a miss waited for. Clocks only advance across I/O, so it
            // counts waiting on the cache and upstream, not CPU.
            headers.set("server-timing", &format!("{};dur={:.0}", served.label(), inv.now() - started))?;
            let age = ((inv.now() - entry.fetched_at) / 1000.0).max(0.0);
            headers.set("x-allthing-age", &format!("{age:.0}"))?;
            if served == Served::StaleIfError {
                headers.set("x-allthing-stale", "1")?;
            }
            // Also protect copies cached before private RPC errors were redacted.
            // Their good counts remain available for the next rebuild to carry.
            let body = if wallets && entry.status == 200 {
                match pages::public_wallet_body(entry.body.as_ref()) {
                    Ok(body) => body.into_owned(),
                    Err(failure) => return error(failure.status, &failure.message, failure.retry_after),
                }
            } else {
                entry.body.as_ref().clone()
            };
            Ok(Response::from_bytes(body)?.with_status(entry.status).with_headers(headers))
        }
        // A preload batch Hypersnap couldn't give, with no copy kept: empty,
        // not an error (`pages::engagement_skipped`), and kept by no one, so
        // it's asked for again.
        Err(failure) if engagement && pages::skips_engagement(&failure) => {
            let headers = Headers::new();
            headers.set("content-type", "application/json; charset=utf-8")?;
            headers.set("cache-control", "no-store")?;
            headers.set("x-allthing-cache", "skipped")?;
            Ok(Response::from_bytes(pages::engagement_skipped(inv.now() as i64))?.with_headers(headers))
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
async fn status(inv: &Invocation, url: &Url, env: &Env) -> Result<Response> {
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
        // Whether Solana wallet counts have a keyed endpoint (the
        // SOLANA_RPC_URL secret): its host only, never the key.
        "solana_rpc": match inv.solana_rpc() {
            Some(endpoint) => serde_json::json!({
                "keyed": true,
                "host": Url::parse(&endpoint).ok().and_then(|u| u.host_str().map(str::to_owned)),
            }),
            None => serde_json::json!({ "keyed": false }),
        },
        // Whether the GIF picker has KLIPY's key (the KLIPY_API_KEY
        // secret), never the key.
        "gifs": { "keyed": auth::klipy_key(env).is_some() },
        // Whether readers can approve an app key to write with (the
        // ALLTHING_FID variable and the ALLTHING_CUSTODY_KEY secret), which
        // account asks, and the custody address of the key set, to check
        // against the account's; never the key.
        "writes": match app_key::app(env) {
            Some(app) => serde_json::json!({
                "ready": true,
                "app_fid": app.fid,
                "address": app.address(),
                "sponsored": app.sponsors(),
            }),
            None => serde_json::json!({ "ready": false }),
        },
        // Whether readers can attach images and videos: the MEDIA bucket
        // (R2) and D1, which counts what they take.
        "media": { "ready": media::ready(env) },
        // Whether signed-in readers' SpacetimeDB identities are attested for
        // messages (the ATTEST_KEY secret), and its public half, which the
        // module pins (spacetime/spacetimedb/src/attest_key.rs); never the key.
        "spacetime": match attest::public_key(env).await {
            Some(public_key) => serde_json::json!({ "attest": true, "public_key": public_key }),
            None => serde_json::json!({ "attest": attest::ready(env) }),
        },
        // Whether votes in Allthing's poll are counted: SESSION_SECRET,
        // which keys the voters, and D1, which keeps them.
        "poll": { "ready": poll::ready(env) },
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

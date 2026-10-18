// SPDX-License-Identifier: AGPL-3.0-or-later
//! Edge caching for API payloads, built on the Workers Cache API:
//!
//! - fresh entries are served as is;
//! - stale entries are served while one background refresh runs
//!   (stale-while-revalidate, via `waitUntil`);
//! - when a rebuild fails, the last good entry keeps being served
//!   (stale-if-error) until it ages out of the cache;
//! - concurrent misses for the same key in an isolate share one build
//!   (request coalescing), and an isolate runs at most one build per key.
//!
//! A build runs in the request that started it, under `waitUntil`, so it
//! finishes even if that request's client goes away. Requests that join it
//! wait on their own timers and read the result from a shared slot: awaiting
//! another request's I/O directly would look like a hang to the runtime, and
//! could outlive that request.
//!
//! The Cache API is per data center and is a no-op on `*.workers.dev`, so it
//! only takes effect on the custom domain.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use allthing_core::api::CachePolicy;
use allthing_core::guard::{Freshness, freshness};
use allthing_data::{Built, Failure, Invocation};
use futures::future::{FutureExt, LocalBoxFuture, Shared};
use worker::{Cache, Delay, Headers, Response};

pub struct Entry {
    pub body: Rc<Vec<u8>>,
    pub status: u16,
    /// Unix milliseconds.
    pub fetched_at: f64,
    pub policy: CachePolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Served {
    Fresh,
    /// Stale, with a background refresh started.
    Revalidating,
    /// Built during this request.
    Built,
    /// Rebuilding failed; the last good copy was served.
    StaleIfError,
}

impl Served {
    pub fn label(self) -> &'static str {
        match self {
            Served::Fresh => "fresh",
            Served::Revalidating => "revalidating",
            Served::Built => "miss",
            Served::StaleIfError => "stale-if-error",
        }
    }
}

pub type Builder = Rc<dyn Fn(Rc<Invocation>, Option<Rc<Vec<u8>>>) -> LocalBoxFuture<'static, Result<Built, Failure>>>;
type Outcome = Result<Rc<Entry>, Failure>;
type SharedBuild = Shared<LocalBoxFuture<'static, Outcome>>;
/// Where a build leaves its outcome for the requests that joined it.
type Slot = Rc<RefCell<Option<Outcome>>>;

/// How long a request waits for another request's build before giving up.
const JOIN_TIMEOUT_MS: u64 = 15_000;
/// A build this old is presumed dead (its request was cancelled, e.g. for
/// exceeding the CPU limit) and no longer blocks its key.
const ABANDONED_MS: f64 = 30_000.0;

struct Running {
    started: f64,
    slot: Slot,
}

thread_local! {
    /// Builds running in this isolate, by cache key.
    static INFLIGHT: RefCell<HashMap<String, Running>> = RefCell::new(HashMap::new());
}

fn header_f64(headers: &Headers, name: &str) -> Option<f64> {
    headers.get(name).ok().flatten()?.parse().ok()
}

/// The cached entry for `key`, whatever its age, without building it.
pub async fn peek(inv: &Invocation, key: &str) -> Option<Entry> {
    lookup(inv, key).await
}

async fn lookup(inv: &Invocation, key: &str) -> Option<Entry> {
    if !inv.take_subrequests(1) {
        return None;
    }
    let mut response = Cache::default().get(key, false).await.ok()??;
    let headers = response.headers();
    let entry_policy = CachePolicy {
        fresh: header_f64(headers, "x-allthing-fresh")? as u32,
        stale: header_f64(headers, "x-allthing-stale-after")? as u32,
        keep: 0,
        browser: header_f64(headers, "x-allthing-browser")? as u32,
    };
    let fetched_at = header_f64(headers, "x-allthing-fetched")?;
    let status = header_f64(headers, "x-allthing-status")? as u16;
    let body = response.bytes().await.ok()?;
    Some(Entry { body: Rc::new(body), status, fetched_at, policy: entry_policy })
}

async fn store(inv: &Invocation, key: &str, built: Built) -> Rc<Entry> {
    let entry =
        Rc::new(Entry { body: Rc::new(built.body), status: built.status, fetched_at: inv.now(), policy: built.policy });
    if inv.take_subrequests(1) {
        let headers = Headers::new();
        let _ = headers.set("content-type", "application/json; charset=utf-8");
        // The edge keeps the entry for `keep`; freshness is decided here.
        let _ = headers.set("cache-control", &format!("public, max-age={}", built.policy.keep.max(1)));
        let _ = headers.set("x-allthing-fetched", &format!("{:.0}", entry.fetched_at));
        let _ = headers.set("x-allthing-status", &entry.status.to_string());
        let _ = headers.set("x-allthing-fresh", &built.policy.fresh.to_string());
        let _ = headers.set("x-allthing-stale-after", &built.policy.stale.to_string());
        let _ = headers.set("x-allthing-browser", &built.policy.browser.to_string());
        if let Ok(response) = Response::from_bytes(entry.body.as_ref().clone()) {
            let _ = Cache::default().put(key, response.with_headers(headers)).await;
        }
    }
    entry
}

/// Start the build of `key` in this request. It also runs under
/// `waitUntil`, so it completes and is cached even if the client goes away.
fn start_build(inv: &Rc<Invocation>, key: &str, build: &Builder, previous: Option<Rc<Vec<u8>>>) -> SharedBuild {
    let slot: Slot = Rc::new(RefCell::new(None));
    let running = Running { started: inv.now(), slot: slot.clone() };
    INFLIGHT.with(|m| m.borrow_mut().insert(key.to_owned(), running));
    let (inv2, key2, build2) = (inv.clone(), key.to_owned(), build.clone());
    let future = async move {
        let outcome = match build2(inv2.clone(), previous).await {
            Ok(built) => Ok(store(&inv2, &key2, built).await),
            Err(failure) => Err(failure),
        };
        *slot.borrow_mut() = Some(outcome.clone());
        INFLIGHT.with(|m| {
            let mut inflight = m.borrow_mut();
            // Unless a newer build replaced this one as abandoned.
            if inflight.get(&key2).is_some_and(|r| Rc::ptr_eq(&r.slot, &slot)) {
                inflight.remove(&key2);
            }
        });
        outcome
    }
    .boxed_local()
    .shared();
    let background = future.clone();
    inv.wait_until(async move {
        let _ = background.await;
    });
    future
}

fn still_building() -> Failure {
    Failure { status: 503, message: "Still gathering this page; try again shortly".into(), retry_after: Some(5) }
}

/// Wait for a build another request started, polling its slot with
/// growing pauses (10 ms up to 200 ms) on this request's own timers.
async fn join_build(slot: Slot) -> Outcome {
    let (mut waited, mut pause) = (0, 10);
    loop {
        if let Some(outcome) = slot.borrow().as_ref() {
            return outcome.clone();
        }
        if waited >= JOIN_TIMEOUT_MS {
            return Err(still_building());
        }
        Delay::from(Duration::from_millis(pause)).await;
        waited += pause;
        pause = (pause * 2).min(200);
    }
}

/// Serve `key` from the edge cache, building it with `build` when missing
/// or expired. The builder receives the previous body, if any, to carry
/// slowly gathered annotations forward.
pub async fn serve(inv: &Rc<Invocation>, key: &str, build: Builder) -> Result<(Rc<Entry>, Served), Failure> {
    let cached = lookup(inv, key).await.map(Rc::new);
    let now = inv.now();
    let running =
        INFLIGHT.with(|m| m.borrow().get(key).filter(|r| now - r.started < ABANDONED_MS).map(|r| r.slot.clone()));
    let previous = cached.as_ref().filter(|e| e.status == 200).map(|e| e.body.clone());
    if let Some(entry) = &cached {
        let age = inv.now() - entry.fetched_at;
        let fresh = f64::from(entry.policy.fresh) * 1000.0;
        let stale = f64::from(entry.policy.stale) * 1000.0;
        match freshness(age, fresh, stale) {
            Freshness::Fresh => return Ok((entry.clone(), Served::Fresh)),
            Freshness::Stale => {
                // Answer now; refresh in the background unless already running.
                if running.is_none() {
                    drop(start_build(inv, key, &build, previous));
                }
                return Ok((entry.clone(), Served::Revalidating));
            }
            Freshness::Expired => {}
        }
    }
    let outcome = match running {
        Some(slot) => join_build(slot).await,
        None => start_build(inv, key, &build, previous).await,
    };
    match outcome {
        Ok(entry) => Ok((entry, Served::Built)),
        Err(failure) => match cached {
            Some(entry) if entry.status == 200 => Ok((entry, Served::StaleIfError)),
            _ => Err(failure),
        },
    }
}

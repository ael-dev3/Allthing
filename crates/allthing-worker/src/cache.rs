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

use std::cell::{Cell, RefCell};
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
    /// Only a newly seeded stale shared Home copy requests another build.
    /// Neither this signal nor its consume-once claim is persisted.
    revalidate_home: bool,
    refresh_claimed: Cell<bool>,
}

impl Entry {
    pub(crate) fn new(body: Rc<Vec<u8>>, status: u16, fetched_at: f64, policy: CachePolicy) -> Self {
        Self { body, status, fetched_at, policy, revalidate_home: false, refresh_claimed: Cell::new(false) }
    }
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
    Some(Entry::new(Rc::new(body), status, fetched_at, entry_policy))
}

async fn store(inv: &Invocation, key: &str, built: Built) -> Rc<Entry> {
    // A stored copy is as old as when it was built, so it is refreshed soon.
    let fetched_at = built.fetched_at.unwrap_or_else(|| inv.now());
    let entry = Rc::new(Entry {
        body: Rc::new(built.body),
        status: built.status,
        fetched_at,
        policy: built.policy,
        revalidate_home: built.revalidate_home,
        refresh_claimed: Cell::new(false),
    });
    put(inv, key, &entry, built.policy.keep).await;
    entry
}

/// Keep `entry` under `key` for `keep` seconds at the edge; whether it was.
pub(crate) async fn put(inv: &Invocation, key: &str, entry: &Entry, keep: u32) -> bool {
    if !inv.take_subrequests(1) {
        return false;
    }
    let headers = Headers::new();
    let _ = headers.set("content-type", "application/json; charset=utf-8");
    // The edge keeps the entry for `keep`; freshness is decided here.
    let _ = headers.set("cache-control", &format!("public, max-age={}", keep.max(1)));
    let _ = headers.set("x-allthing-fetched", &format!("{:.0}", entry.fetched_at));
    let _ = headers.set("x-allthing-status", &entry.status.to_string());
    let _ = headers.set("x-allthing-fresh", &entry.policy.fresh.to_string());
    let _ = headers.set("x-allthing-stale-after", &entry.policy.stale.to_string());
    let _ = headers.set("x-allthing-browser", &entry.policy.browser.to_string());
    let Ok(response) = Response::from_bytes(entry.body.as_ref().clone()) else { return false };
    Cache::default().put(key, response.with_headers(headers)).await.is_ok()
}

/// How long an expired entry is kept for the rebuild that replaces it to
/// carry forward: a day, as every payload that can be expired is kept.
const EXPIRED_KEEP: u32 = 86_400;

/// Make the entry for `key` due for a rebuild, if there is one and `ours`
/// says it may be: it is kept as it is, but dated long ago, so the next
/// request builds it anew, carrying it forward (and serving it still if
/// that build fails), as for any expired entry. A cache read, and a write
/// when there is one; whether it was expired.
pub async fn expire(inv: &Invocation, key: &str, ours: impl Fn(&Entry) -> bool) -> bool {
    let Some(entry) = lookup(inv, key).await else { return false };
    if !ours(&entry) {
        return false;
    }
    put(inv, key, &Entry { fetched_at: 0.0, ..entry }, EXPIRED_KEEP).await
}

/// Start the build of `key` in this request. It also runs under
/// `waitUntil`, so it completes and is cached even if the client goes away.
fn start_build(
    inv: &Rc<Invocation>,
    key: &str,
    build: &Builder,
    previous: Option<Rc<Vec<u8>>>,
    allow_shared_home: bool,
) -> SharedBuild {
    let slot: Slot = Rc::new(RefCell::new(None));
    let running = Running { started: inv.now(), slot: slot.clone() };
    INFLIGHT.with(|m| m.borrow_mut().insert(key.to_owned(), running));
    let (inv2, key2, build2) = (inv.clone(), key.to_owned(), build.clone());
    let future = async move {
        let outcome = match build2(inv2.clone(), previous).await {
            Ok(built) if built.revalidate_home && (!allow_shared_home || !valid_home_refresh(&built, inv2.now())) => {
                Err(Failure::internal("invalid shared Home refresh"))
            }
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

fn valid_home_refresh(built: &Built, now: f64) -> bool {
    built.fetched_at.is_some_and(|at| valid_home_age(built.status, built.policy, at, now))
}

fn valid_home_age(status: u16, policy: CachePolicy, at: f64, now: f64) -> bool {
    let age = now - at;
    status == 200
        && policy == allthing_core::api::HOME_CACHE
        && at.is_finite()
        && at > 0.0
        && (f64::from(policy.fresh) * 1000.0..f64::from(policy.stale) * 1000.0).contains(&age)
}

/// One old-slot result may reach several waiting requests. Consume its
/// claim before checking the current build, including if that build fails
/// before a later waiter resumes. The request claiming the refresh owns
/// its I/O; no request awaits another request's platform future.
fn start_home_refresh(inv: &Rc<Invocation>, key: &str, build: &Builder, entry: &Entry) -> bool {
    if !entry.revalidate_home || entry.refresh_claimed.replace(true) {
        return false;
    }
    let running = INFLIGHT.with(|m| m.borrow().get(key).is_some_and(|r| inv.now() - r.started < ABANDONED_MS));
    if running {
        return false;
    }
    drop(start_build(inv, key, build, Some(entry.body.clone()), false));
    true
}

fn serve_shared_home(
    inv: &Rc<Invocation>,
    key: &str,
    build: &Builder,
    entry: Rc<Entry>,
) -> Result<(Rc<Entry>, Served), Failure> {
    start_home_refresh(inv, key, build, &entry);
    // A slow seed put or late old-slot waiter can cross the existing stale
    // deadline. Keep refreshing, but do not serve that now-expired copy.
    if !valid_home_age(entry.status, entry.policy, entry.fetched_at, inv.now()) {
        return Err(still_building());
    }
    Ok((entry, Served::Revalidating))
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
                    drop(start_build(inv, key, &build, previous, false));
                }
                return Ok((entry.clone(), Served::Revalidating));
            }
            Freshness::Expired => {}
        }
    }
    let outcome = match running {
        Some(slot) => join_build(slot).await,
        None => start_build(inv, key, &build, previous, true).await,
    };
    match outcome {
        Ok(entry) if entry.revalidate_home => serve_shared_home(inv, key, &build, entry),
        Ok(entry) => Ok((entry, Served::Built)),
        Err(failure) => match cached {
            Some(entry) if entry.status == 200 => Ok((entry, Served::StaleIfError)),
            _ => Err(failure),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use allthing_data::{FetchError, Host, HttpRequest, HttpResponse};
    use futures::channel::oneshot;

    const NOW: f64 = 1_800_000_000_000.0;

    #[derive(Default)]
    struct Runtime {
        now: Cell<f64>,
        background: RefCell<Vec<LocalBoxFuture<'static, ()>>>,
    }

    impl Host for Runtime {
        fn now(&self) -> f64 {
            self.now.get()
        }
        fn fetch(&self, _: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            panic!("the test builder must not reach the network")
        }
        fn spawn(&self, future: LocalBoxFuture<'static, ()>) {
            self.background.borrow_mut().push(future);
        }
    }

    fn runtime() -> (Rc<Runtime>, Rc<Invocation>) {
        let runtime = Rc::new(Runtime { now: Cell::new(NOW), ..Runtime::default() });
        let invocation = Rc::new(Invocation::new(runtime.clone(), 45));
        (runtime, invocation)
    }

    fn seed(age: f64) -> Rc<Entry> {
        Rc::new(Entry {
            body: Rc::new(br#"{"source":"shared"}"#.to_vec()),
            status: 200,
            fetched_at: NOW - age,
            policy: allthing_core::api::HOME_CACHE,
            revalidate_home: true,
            refresh_claimed: Cell::new(false),
        })
    }

    fn finish_background(runtime: &Runtime) {
        for task in runtime.background.take() {
            assert!(task.now_or_never().is_some());
        }
    }

    #[test]
    fn shared_home_refresh_claim_survives_fast_failure_and_a_late_old_slot_waiter() {
        let (runtime, inv) = runtime();
        let entry = seed(120_000.0);
        let old_slot = Rc::new(RefCell::new(Some(Ok(entry.clone()))));
        let calls = Rc::new(Cell::new(0));
        let observed = calls.clone();
        let build: Builder = Rc::new(move |_, previous| {
            observed.set(observed.get() + 1);
            assert_eq!(previous.as_deref().map(Vec::as_slice), Some(br#"{"source":"shared"}"#.as_slice()));
            async { Err(Failure::internal("failed refresh")) }.boxed_local()
        });
        let (_, served) = serve_shared_home(&inv, "fast-failure", &build, entry.clone()).unwrap();
        assert_eq!(served, Served::Revalidating);
        finish_background(&runtime);
        assert_eq!(calls.get(), 1);
        assert!(!INFLIGHT.with(|m| m.borrow().contains_key("fast-failure")));

        let late = join_build(old_slot).now_or_never().unwrap().unwrap();
        assert!(Rc::ptr_eq(&entry, &late));
        assert_eq!(serve_shared_home(&inv, "fast-failure", &build, late).unwrap().1, Served::Revalidating);
        assert!(runtime.background.borrow().is_empty());
        assert_eq!(calls.get(), 1, "a completed failed refresh must not reopen the old result's claim");
    }

    #[test]
    fn another_inflight_refresh_consumes_the_old_result_claim_without_duplicating_work() {
        let (runtime, inv) = runtime();
        let (release, gate) = oneshot::channel();
        let gate = Rc::new(RefCell::new(Some(gate)));
        let calls = Rc::new(Cell::new(0));
        let observed = calls.clone();
        let build: Builder = Rc::new(move |_, _| {
            observed.set(observed.get() + 1);
            let gate = gate.borrow_mut().take().unwrap();
            async move {
                let _ = gate.await;
                Err(Failure::internal("failed existing refresh"))
            }
            .boxed_local()
        });
        let mut running = Box::pin(start_build(&inv, "already-running", &build, None, true));
        assert!(running.as_mut().now_or_never().is_none());
        let old = seed(120_000.0);
        assert!(!start_home_refresh(&inv, "already-running", &build, &old));
        release.send(()).unwrap();
        assert!(running.now_or_never().unwrap().is_err());
        finish_background(&runtime);
        assert!(!start_home_refresh(&inv, "already-running", &build, &old));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_home_seed_expiring_during_store_or_before_a_late_waiter_is_not_served() {
        let (runtime, inv) = runtime();
        let old = seed(599_999.0);
        let build: Builder = Rc::new(|_, _| async { Err(Failure::internal("held source")) }.boxed_local());
        assert_eq!(serve_shared_home(&inv, "expires", &build, old.clone()).unwrap().1, Served::Revalidating);
        finish_background(&runtime);
        runtime.now.set(NOW + 1.0);
        let failure = serve_shared_home(&inv, "expires", &build, old.clone()).err().unwrap();
        assert_eq!((failure.status, failure.retry_after), (503, Some(5)));
        assert_eq!(old.fetched_at, NOW - 599_999.0);
        assert!(runtime.background.borrow().is_empty());
        let delayed_put = seed(599_999.0);
        let failure = serve_shared_home(&inv, "delayed-put", &build, delayed_put).err().unwrap();
        assert_eq!((failure.status, failure.retry_after), (503, Some(5)));
        finish_background(&runtime);
    }

    #[test]
    fn shared_home_revalidation_is_one_hop_and_never_implicit_for_other_snapshots() {
        let (runtime, inv) = runtime();
        let build: Builder = Rc::new(|_, _| {
            let mut built = Built::json(&"shared", allthing_core::api::HOME_CACHE).unwrap();
            built.fetched_at = Some(NOW - 120_000.0);
            built.revalidate_home = true;
            async move { Ok(built) }.boxed_local()
        });
        assert!(start_build(&inv, "one-hop", &build, None, false).now_or_never().unwrap().is_err());
        finish_background(&runtime);
        for policy in [allthing_core::api::LATEST_CACHE, allthing_core::api::ACTIVITY_CACHE] {
            let built = Built::json(&"snapshot", policy).unwrap();
            assert!(!built.revalidate_home);
            assert!(!valid_home_refresh(&built, NOW));
        }
        for (age, valid) in [(59_999.0, false), (60_000.0, true), (599_999.0, true), (600_000.0, false)] {
            assert_eq!(valid_home_age(200, allthing_core::api::HOME_CACHE, NOW - age, NOW), valid);
        }
        for at in [f64::NAN, f64::INFINITY, 0.0, NOW + 1.0] {
            assert!(!valid_home_age(200, allthing_core::api::HOME_CACHE, at, NOW));
        }
    }
}

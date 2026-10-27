// SPDX-License-Identifier: AGPL-3.0-or-later
use std::cell::{Cell, RefCell};

use futures::channel::oneshot;
use futures::future::{FutureExt, LocalBoxFuture};
use serde_json::json;

use super::*;
use crate::indexer::{Index, IndexError, Saved};
use crate::{FetchError, Host, HttpRequest, HttpResponse};

const NOW: f64 = 1_800_000_000_000.0;

#[derive(Default)]
struct State {
    copy: RefCell<Option<Snapshot>>,
    reads: RefCell<Vec<&'static str>>,
    writes: RefCell<Vec<Snapshot>>,
    read_fails: Cell<bool>,
    write_fails: Cell<bool>,
    write_waits: Cell<bool>,
    background: RefCell<Vec<LocalBoxFuture<'static, ()>>>,
    now: Cell<f64>,
    read_delay: Cell<f64>,
    source_gate: RefCell<Option<oneshot::Receiver<()>>>,
}

impl Index for State {
    fn ranking(&self, _: Span) -> LocalBoxFuture<'static, Result<Option<Ranking>, IndexError>> {
        async { Ok(None) }.boxed_local()
    }
    fn load(&self) -> LocalBoxFuture<'static, Result<Saved, IndexError>> {
        async { Ok(Saved::default()) }.boxed_local()
    }
    fn save(
        &self,
        _: Vec<allthing_core::index::Entry>,
        _: Option<Span>,
        _: String,
        _: f64,
    ) -> LocalBoxFuture<'static, Result<(), IndexError>> {
        async { Ok(()) }.boxed_local()
    }
    fn snapshot(&self, name: &'static str) -> LocalBoxFuture<'static, Result<Option<Snapshot>, IndexError>> {
        self.reads.borrow_mut().push(name);
        let result = if name == HOME_SNAPSHOT {
            self.now.set(self.now.get() + self.read_delay.get());
            if self.read_fails.get() { Err(IndexError("read failed".into())) } else { Ok(self.copy.borrow().clone()) }
        } else {
            Ok(None)
        };
        async move { result }.boxed_local()
    }
    fn save_snapshot(&self, name: &'static str, copy: Snapshot) -> LocalBoxFuture<'static, Result<(), IndexError>> {
        assert_eq!(name, HOME_SNAPSHOT, "no private or differently windowed snapshot may be saved");
        self.writes.borrow_mut().push(copy.clone());
        if self.write_waits.get() {
            return futures::future::pending().boxed_local();
        }
        let result = if self.write_fails.get() {
            Err(IndexError("write failed".into()))
        } else {
            *self.copy.borrow_mut() = Some(copy);
            Ok(())
        };
        async move { result }.boxed_local()
    }
}

struct Source {
    state: Rc<State>,
    requests: RefCell<Vec<String>>,
    no_index: bool,
    fail: bool,
    large: bool,
}

impl Host for Source {
    fn now(&self) -> f64 {
        self.state.now.get()
    }
    fn index(&self) -> Option<Rc<dyn Index>> {
        (!self.no_index).then(|| self.state.clone() as Rc<dyn Index>)
    }
    fn spawn(&self, future: LocalBoxFuture<'static, ()>) {
        self.state.background.borrow_mut().push(future);
    }
    fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
        self.requests.borrow_mut().push(request.url.clone());
        let gate = self.state.source_gate.borrow_mut().take();
        let body = if request.url.contains("/feed/trending?") {
            json!({"casts": [{"hash": "0xfresh", "author": {"fid": 0, "username": "fresh"},
                "text": if self.large { "x".repeat(HOME_SNAPSHOT_MAX_BYTES) } else { "fresh source".into() },
                "replies": {"count": 1},
                "timestamp": "2027-01-15T07:59:00Z"}]})
        } else if request.url.contains("/feed/following?") {
            json!({"casts": [], "next": {}})
        } else {
            panic!("unexpected upstream request: {}", request.url);
        };
        let response = HttpResponse {
            status: if self.fail { 500 } else { 200 },
            retry_after: None,
            body: (!self.fail).then(|| serde_json::to_vec(&body).unwrap()),
        };
        async move {
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            Ok(response)
        }
        .boxed_local()
    }
}

fn source(copy: Option<Snapshot>) -> Rc<Source> {
    Rc::new(Source {
        state: Rc::new(State { copy: RefCell::new(copy), now: Cell::new(NOW), ..State::default() }),
        requests: RefCell::new(Vec::new()),
        no_index: false,
        fail: false,
        large: false,
    })
}

fn cast(hash: &str, ts: i64) -> Cast {
    serde_json::from_value(
        json!({"hash":hash,"author":{"fid":1,"username":"one","display_name":"One"},"text":"shared", "ts":ts}),
    )
    .unwrap()
}

fn snapshot(age: f64, timestamps: &[i64]) -> Snapshot {
    let built = NOW - age;
    let payload = HomePayload {
        pool: timestamps.iter().enumerate().map(|(i, &ts)| cast(&format!("0x{i}"), ts)).collect(),
        notes: Annotations::default(),
        meta: Meta { backend: Backend::Hypersnap, fetched_at: built as i64, stale: false },
        index: None,
        apps_24h: None,
        poll: None,
    };
    // Whitespace makes exact-byte preservation observable, beyond parsed equality.
    Snapshot { built, body: serde_json::to_string_pretty(&payload).unwrap() + "\n" }
}

fn build(
    source: &Rc<Source>,
    budget: u32,
    offset: u32,
    window: TrendingWindow,
    previous: Option<Rc<Vec<u8>>>,
) -> (Rc<Invocation>, Result<Built, Failure>) {
    let inv = Rc::new(Invocation::new(source.clone(), budget));
    let built = home(inv.clone(), offset, window, previous).now_or_never().unwrap();
    (inv, built)
}

fn finish_save(host: &Source) {
    for future in host.state.background.take() {
        assert!(future.now_or_never().is_some());
    }
}

#[test]
fn cold_copy_uses_one_read_no_upstream_and_preserves_original_bytes_and_age() {
    let copy = snapshot(10_000.0, &[(NOW / 1000.0) as i64 - 10]);
    let host = source(Some(copy.clone()));
    for now in [NOW, NOW + 1000.0] {
        host.state.now.set(now);
        let (inv, built) = build(&host, 3, 0, TrendingWindow::Day, None);
        let built = built.unwrap();
        assert_eq!(built.body, copy.body.as_bytes());
        assert_eq!((built.fetched_at, built.status, built.policy), (Some(copy.built), 200, api::HOME_CACHE));
        assert!(!built.revalidate_home);
        assert_eq!(inv.remaining_subrequests(), 2, "the edge-cache put still fits");
    }
    assert_eq!(*host.state.reads.borrow(), [HOME_SNAPSHOT, HOME_SNAPSHOT]);
    assert!(host.requests.borrow().is_empty() && host.state.writes.borrow().is_empty());
    assert_eq!(*host.state.copy.borrow(), Some(copy));
}

#[test]
fn copy_filters_both_current_day_edges_without_rewriting_shared_source() {
    let now = (NOW / 1000.0) as i64;
    let copy = snapshot(10_000.0, &[now - 86_400, now - 86_399, now, now + 1, i64::MAX]);
    let host = source(Some(copy.clone()));
    let (_, built) = build(&host, 3, 0, TrendingWindow::Day, None);
    let built = built.unwrap();
    let payload: HomePayload = serde_json::from_slice(&built.body).unwrap();
    assert_eq!(payload.pool.iter().map(|c| c.ts).collect::<Vec<_>>(), [now - 86_399, now]);
    assert_eq!((payload.meta.fetched_at as f64, built.fetched_at), (copy.built, Some(copy.built)));
    assert_eq!(*host.state.copy.borrow(), Some(copy));
    assert!(host.state.writes.borrow().is_empty() && host.requests.borrow().is_empty());
}

#[test]
fn bounds_use_the_clock_after_the_shared_read_finishes() {
    let now = (NOW / 1000.0) as i64;
    let host = source(Some(snapshot(1000.0, &[now - 86_399, now])));
    host.state.read_delay.set(2000.0);
    let (_, built) = build(&host, 3, 0, TrendingWindow::Day, None);
    let payload: HomePayload = serde_json::from_slice(&built.unwrap().body).unwrap();
    assert_eq!(payload.pool.iter().map(|c| c.ts).collect::<Vec<_>>(), [now]);
}

#[test]
fn invalid_or_expired_shared_copies_fall_back_to_normal_builds() {
    let valid = snapshot(1000.0, &[(NOW / 1000.0) as i64 - 10]);
    let changed = |field: &str, value| {
        let mut copy = valid.clone();
        let mut body: serde_json::Value = serde_json::from_str(&copy.body).unwrap();
        body[field] = value;
        copy.body = body.to_string();
        copy
    };
    let cases = [
        snapshot(600_000.0, &[]),
        snapshot(-1.0, &[]),
        Snapshot { built: f64::NAN, ..valid.clone() },
        Snapshot { built: f64::INFINITY, ..valid.clone() },
        Snapshot { built: 0.0, ..valid.clone() },
        Snapshot { body: "{".into(), ..valid.clone() },
        Snapshot { body: valid.body.clone() + &" ".repeat(HOME_SNAPSHOT_MAX_BYTES + 1), ..valid.clone() },
        changed("meta", json!({"backend":"hypersnap","fetched_at": NOW as i64,"stale":false})),
        changed("meta", json!({"backend":"hypersnap","fetched_at":valid.built as i64,"stale":true})),
        changed("index", json!({"casts":1,"since":1,"ranked_at":1})),
    ];
    for copy in cases {
        let host = source(Some(copy));
        let (_, built) = build(&host, 45, 0, TrendingWindow::Day, None);
        let built = built.unwrap();
        finish_save(&host);
        let payload: HomePayload = serde_json::from_slice(&built.body).unwrap();
        assert_eq!(payload.pool[0].hash, "0xfresh");
        assert_eq!(host.requests.borrow().len(), 1);
        let writes = host.state.writes.borrow();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].body.as_bytes(), built.body);
        assert_eq!(writes[0].built, payload.meta.fetched_at as f64);
    }
    assert!(!shared_home_snapshot(snapshot(59_999.0, &[]), NOW).unwrap().revalidate_home);
}

#[test]
fn shared_read_failure_does_not_fail_a_valid_home_build() {
    let host = source(None);
    host.state.read_fails.set(true);
    assert!(build(&host, 45, 0, TrendingWindow::Day, None).1.is_ok());
    finish_save(&host);
    assert_eq!(host.requests.borrow().len(), 1);
    assert_eq!(host.state.writes.borrow().len(), 1);
}

#[test]
fn failed_write_or_failed_full_build_keeps_the_previous_shared_copy() {
    let old = snapshot(600_000.0, &[]);
    let host = source(Some(old.clone()));
    host.state.write_fails.set(true);
    assert!(build(&host, 45, 0, TrendingWindow::Day, None).1.is_ok());
    finish_save(&host);
    assert_eq!(*host.state.copy.borrow(), Some(old.clone()));
    let mut host = source(Some(old.clone()));
    Rc::get_mut(&mut host).unwrap().fail = true;
    assert!(build(&host, 45, 0, TrendingWindow::Day, None).1.is_err());
    assert!(host.state.writes.borrow().is_empty());
    assert_eq!(*host.state.copy.borrow(), Some(old));
}

#[test]
fn a_scarce_build_budget_charges_the_shared_read_and_reserved_write_and_keeps_cache_put() {
    let host = source(None);
    let (inv, result) = build(&host, 8, 0, TrendingWindow::Day, None);
    assert!(result.is_ok());
    finish_save(&host);
    assert_eq!(*host.state.reads.borrow(), [HOME_SNAPSHOT, LATEST_SNAPSHOT, crate::activity::SUMMARY]);
    assert_eq!(host.state.writes.borrow().len(), 1);
    assert_eq!(host.requests.borrow().len(), 1);
    // Three snapshot reads, poll/reaction reads, one upstream, reserved write.
    assert_eq!(inv.remaining_subrequests(), 1);
    assert!(inv.take_subrequests(1), "the existing edge cache can still store the response");
    let host = source(None);
    assert!(build(&host, 1, 0, TrendingWindow::Day, None).1.is_ok());
    assert!(host.state.writes.borrow().is_empty() && host.state.reads.borrow().is_empty());
    assert_eq!(host.requests.borrow().len(), 1, "the optional snapshot must not consume the only normal-source slot");
    let host = source(None);
    assert!(build(&host, 0, 0, TrendingWindow::Day, None).1.is_err());
    assert!(host.state.writes.borrow().is_empty() && host.requests.borrow().is_empty());
}

#[test]
fn one_through_four_slots_preserve_normal_source_availability_without_an_optional_save() {
    for budget in 1..=4 {
        let host = source(None);
        let baseline = source(None);
        // An existing edge body bypasses shared lookup, the original build path.
        let (before, old) = build(&baseline, budget, 0, TrendingWindow::Day, Some(Rc::new(b"{}".to_vec())));
        let (after, new) = build(&host, budget, 0, TrendingWindow::Day, None);
        assert_eq!(new.unwrap().body, old.unwrap().body, "budget {budget}");
        assert_eq!(host.requests.borrow().len(), 1, "the normal source still runs with budget {budget}");
        assert_eq!(after.remaining_subrequests(), before.remaining_subrequests());
        assert_eq!(host.state.reads.borrow().contains(&HOME_SNAPSHOT), budget > 2);
        assert!(host.state.writes.borrow().is_empty() && host.state.background.borrow().is_empty());
    }
}

#[test]
fn pending_background_persistence_cannot_delay_the_response_or_reset_its_age() {
    let old = snapshot(600_000.0, &[]);
    let host = source(Some(old.clone()));
    host.state.write_waits.set(true);
    let (inv, result) = build(&host, 8, 0, TrendingWindow::Day, None);
    let built = result.unwrap();
    assert_eq!(built.fetched_at, Some(NOW));
    assert_eq!(inv.remaining_subrequests(), 1);
    assert!(host.state.writes.borrow().is_empty(), "the response completed before the save was polled");
    let save = host.state.background.borrow_mut().pop().unwrap();
    host.state.now.set(NOW + 30_000.0);
    assert!(save.now_or_never().is_none(), "simulate a D1 write that remains pending");
    assert_eq!(host.state.writes.borrow()[0].built, NOW, "background scheduling never updates source age");
    assert_eq!(host.state.writes.borrow()[0].body.as_bytes(), built.body);
    assert_eq!(*host.state.copy.borrow(), Some(old));
    assert_eq!(built.fetched_at, Some(NOW));
}

#[test]
fn expired_retained_edges_can_use_a_newer_shared_copy_but_swr_refreshes_still_build() {
    let fresh = snapshot(1000.0, &[(NOW / 1000.0) as i64 - 10]);
    for age in [600_000.0, 600_001.0, 3_600_000.0] {
        let previous = snapshot(age, &[]);
        let host = source(Some(fresh.clone()));
        let (inv, result) = build(&host, 3, 0, TrendingWindow::Day, Some(Rc::new(previous.body.into_bytes())));
        let built = result.unwrap();
        assert_eq!(built.body, fresh.body.as_bytes());
        assert_eq!(built.fetched_at, Some(fresh.built));
        assert_eq!(inv.remaining_subrequests(), 2);
        assert_eq!(*host.state.reads.borrow(), [HOME_SNAPSHOT]);
        assert!(host.requests.borrow().is_empty() && host.state.background.borrow().is_empty());
    }
    for age in [60_000.0, 120_000.0, 599_999.0] {
        let previous = snapshot(age, &[]);
        let host = source(Some(fresh.clone()));
        let (_, result) = build(&host, 45, 0, TrendingWindow::Day, Some(Rc::new(previous.body.into_bytes())));
        let payload: HomePayload = serde_json::from_slice(&result.unwrap().body).unwrap();
        assert_eq!(payload.pool[0].hash, "0xfresh");
        assert!(!host.state.reads.borrow().contains(&HOME_SNAPSHOT));
        assert_eq!(host.requests.borrow().len(), 1);
        finish_save(&host);
    }
}

#[test]
fn untrustworthy_previous_metadata_cannot_short_circuit_a_normal_build() {
    let valid = snapshot(600_001.0, &[]);
    let oversized = valid.body.clone() + &" ".repeat(HOME_SNAPSHOT_MAX_BYTES + 1);
    let zero = snapshot(NOW, &[]);
    for previous in ["{".to_owned(), snapshot(-1.0, &[]).body, oversized, zero.body] {
        let host = source(Some(snapshot(1000.0, &[])));
        let (_, result) = build(&host, 45, 0, TrendingWindow::Day, Some(Rc::new(previous.into_bytes())));
        assert!(result.is_ok());
        assert!(!host.state.reads.borrow().contains(&HOME_SNAPSHOT));
        assert_eq!(host.requests.borrow().len(), 1);
    }
}

#[test]
fn refreshed_edges_other_windows_offsets_and_following_never_read_the_shared_home_copy() {
    let copy = snapshot(1000.0, &[(NOW / 1000.0) as i64]);
    let host = source(Some(copy.clone()));
    assert!(build(&host, 45, 0, TrendingWindow::Day, Some(Rc::new(copy.body.into_bytes()))).1.is_ok());
    finish_save(&host);
    assert!(!host.state.reads.borrow().contains(&HOME_SNAPSHOT));
    assert_eq!(host.state.writes.borrow().len(), 1, "successful normal refreshes may renew public Day Home");
    for (offset, window) in [
        (100, TrendingWindow::Day),
        (0, TrendingWindow::Hour),
        (0, TrendingWindow::SixHours),
        (0, TrendingWindow::TwelveHours),
        (0, TrendingWindow::Week),
        (0, TrendingWindow::Month),
        (0, TrendingWindow::Year),
        (0, TrendingWindow::All),
    ] {
        let host = source(None);
        assert!(!build(&host, 45, offset, window, None).1.unwrap().revalidate_home);
        assert!(!host.state.reads.borrow().contains(&HOME_SNAPSHOT));
        assert!(host.state.writes.borrow().is_empty());
    }
    let host = source(None);
    assert!(
        !following(Rc::new(Invocation::new(host.clone(), 45)), 123, None, None)
            .now_or_never()
            .unwrap()
            .unwrap()
            .revalidate_home
    );
    assert!(!host.state.reads.borrow().contains(&HOME_SNAPSHOT));
    assert!(host.state.writes.borrow().is_empty());
}

#[test]
fn without_storage_or_with_an_oversized_build_the_response_still_serves() {
    let mut host = source(None);
    Rc::get_mut(&mut host).unwrap().no_index = true;
    let (inv, result) = build(&host, 2, 0, TrendingWindow::Day, None);
    assert!(result.is_ok());
    assert!(host.state.reads.borrow().is_empty() && host.state.writes.borrow().is_empty());
    assert_eq!(inv.remaining_subrequests(), 1);
    let old = snapshot(600_000.0, &[]);
    let mut host = source(Some(old.clone()));
    Rc::get_mut(&mut host).unwrap().large = true;
    let (_, result) = build(&host, 45, 0, TrendingWindow::Day, None);
    assert!(result.unwrap().body.len() > HOME_SNAPSHOT_MAX_BYTES);
    assert!(host.state.writes.borrow().is_empty());
    assert_eq!(*host.state.copy.borrow(), Some(old));
}

#[test]
fn stale_shared_copies_request_one_refresh_only_within_the_existing_stale_interval() {
    for age in [60_000.0, 120_000.0, 599_999.0] {
        let copy = snapshot(age, &[(NOW / 1000.0) as i64 - 10]);
        let host = source(Some(copy.clone()));
        let (inv, result) = build(&host, 3, 0, TrendingWindow::Day, None);
        let built = result.unwrap();
        assert!(built.revalidate_home);
        assert_eq!(built.body, copy.body.as_bytes());
        assert_eq!(built.fetched_at, Some(copy.built));
        assert_eq!(inv.remaining_subrequests(), 2);
        assert!(host.requests.borrow().is_empty() && host.state.background.borrow().is_empty());
    }
    let host = source(Some(snapshot(599_999.0, &[])));
    host.state.read_delay.set(2.0);
    let (_, result) = build(&host, 45, 0, TrendingWindow::Day, None);
    let built = result.unwrap();
    assert!(!built.revalidate_home, "a copy expiring during the D1 read cannot seed an edge");
    assert_eq!(host.requests.borrow().len(), 1);
    assert_eq!(built.fetched_at, Some(NOW + 2.0));
}

#[test]
fn stale_response_precedes_delayed_real_source_revalidation_without_resetting_its_age() {
    let old = snapshot(120_000.0, &[(NOW / 1000.0) as i64 - 10]);
    let host = source(Some(old.clone()));
    let (release, gate) = oneshot::channel();
    *host.state.source_gate.borrow_mut() = Some(gate);
    let (inv, result) = build(&host, 45, 0, TrendingWindow::Day, None);
    let seeded = result.unwrap();
    assert!(seeded.revalidate_home);
    assert_eq!(seeded.fetched_at, Some(old.built));
    assert_eq!(seeded.body, old.body.as_bytes());
    assert!(host.requests.borrow().is_empty(), "the cold response does not await the source");
    assert_eq!(inv.remaining_subrequests(), 44);
    assert!(inv.take_subrequests(1), "charge the initial edge seed put to the same invocation");

    let previous = Rc::new(seeded.body.clone());
    let mut refresh = Box::pin(home(inv.clone(), 0, TrendingWindow::Day, Some(previous)));
    assert!(refresh.as_mut().now_or_never().is_none(), "the actual source request remains held");
    assert_eq!(host.requests.borrow().len(), 1);
    assert_eq!(host.state.reads.borrow().iter().filter(|&&name| name == HOME_SNAPSHOT).count(), 1);
    assert!(host.state.writes.borrow().is_empty());
    assert_eq!(*host.state.copy.borrow(), Some(old.clone()));

    host.state.now.set(NOW + 20_000.0);
    release.send(()).unwrap();
    let built = refresh.now_or_never().unwrap().unwrap();
    assert!(!built.revalidate_home);
    let payload: HomePayload = serde_json::from_slice(&built.body).unwrap();
    assert_eq!(payload.pool[0].hash, "0xfresh");
    assert_eq!(built.fetched_at, Some(NOW + 20_000.0));
    assert_eq!(payload.meta.fetched_at as f64, NOW + 20_000.0);
    assert_eq!(seeded.fetched_at, Some(old.built), "refreshing cannot make the returned source younger");
    assert_eq!(inv.remaining_subrequests(), 37);
    assert!(inv.take_subrequests(1), "the refresh's edge put also fits the original budget");
    finish_save(&host);
    assert_eq!(host.state.writes.borrow().len(), 1);
    assert_eq!(host.state.copy.borrow().as_ref().unwrap().body.as_bytes(), built.body);
}

#[test]
fn failed_delayed_revalidation_cannot_replace_the_shared_source() {
    let old = snapshot(120_000.0, &[(NOW / 1000.0) as i64 - 10]);
    let mut host = source(Some(old.clone()));
    Rc::get_mut(&mut host).unwrap().fail = true;
    let (release, gate) = oneshot::channel();
    *host.state.source_gate.borrow_mut() = Some(gate);
    let (inv, result) = build(&host, 45, 0, TrendingWindow::Day, None);
    let seeded = result.unwrap();
    assert!(seeded.revalidate_home);
    let mut refresh = Box::pin(home(inv, 0, TrendingWindow::Day, Some(Rc::new(seeded.body))));
    assert!(refresh.as_mut().now_or_never().is_none());
    release.send(()).unwrap();
    assert!(refresh.now_or_never().unwrap().is_err());
    assert_eq!(*host.state.copy.borrow(), Some(old));
    assert!(host.state.writes.borrow().is_empty());
    assert_eq!(host.state.reads.borrow().iter().filter(|&&name| name == HOME_SNAPSHOT).count(), 1);
}

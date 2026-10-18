// SPDX-License-Identifier: AGPL-3.0-or-later
use allthing_core::{model::Cast, normalize};
use allthing_data::{FetchError, Host, HttpRequest, HttpResponse, Invocation, miniapps};
use futures::future::{FutureExt, LocalBoxFuture, poll_fn};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::future::Future;
use std::rc::Rc;
use std::task::{Context, Poll};

struct Pages {
    now: Cell<f64>,
    requests: Cell<usize>,
    ready: Rc<Cell<bool>>,
}
impl Pages {
    fn new(ready: bool) -> Self {
        Self { now: Cell::new(1_800_000_000_000.0), requests: Cell::new(0), ready: Rc::new(Cell::new(ready)) }
    }
}
impl Host for Pages {
    fn now(&self) -> f64 {
        self.now.get()
    }
    fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    fn fetch(&self, _: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
        self.requests.set(self.requests.get() + 1);
        let ready = self.ready.clone();
        poll_fn(move |_| {
            if ready.get() {
                Poll::Ready(Ok(HttpResponse { status: 404, retry_after: None, body: None }))
            } else {
                Poll::Pending
            }
        })
        .boxed_local()
    }
}
fn casts(prefix: &str, start: usize) -> Vec<Cast> {
    (start..start + 12)
        .map(|n| {
            normalize::cast(
                &serde_json::from_value(serde_json::json!({
                    "hash":format!("0x{n:040x}"),"author":{"fid":1,"username":"fixture"},"text":"Ordinary link",
                    "timestamp":"2026-10-18T10:00:00Z","embeds":[{"url":format!("https://{prefix}.example/{n}")}]
                }))
                .unwrap(),
            )
        })
        .collect()
}

#[test]
fn budget_is_shared_between_root_and_quoted_link_checks() {
    let host = Rc::new(Pages::new(true));
    let inv = Invocation::new(host.clone(), 45);
    let root = casts("shared-budget", 0);
    let quoted = casts("shared-budget", 12);
    miniapps::check(&inv, &root, &BTreeMap::new()).now_or_never().unwrap();
    host.now.set(host.now.get() + 6000.0); // refill the upstream bucket
    miniapps::check(&inv, &quoted, &BTreeMap::new()).now_or_never().unwrap();
    assert_eq!(host.requests.get(), 12, "one build checked more than twelve links");
    host.now.set(host.now.get() + 6000.0);
    let next = Invocation::new(host.clone(), 45);
    assert_eq!(miniapps::check(&next, &quoted, &BTreeMap::new()).now_or_never().unwrap().len(), 12);
    assert_eq!(host.requests.get(), 24, "deferred links should remain eligible in a later build");
}

#[test]
fn budget_exhaustion_does_not_poison_deferred_links_for_an_hour() {
    let host = Rc::new(Pages::new(false));
    let inv = Invocation::new(host.clone(), 45);
    let pool = casts("budget-race", 0);
    let known = BTreeMap::new();
    let mut check = Box::pin(miniapps::check(&inv, &pool, &known));
    assert!(check.as_mut().poll(&mut Context::from_waker(futures::task::noop_waker_ref())).is_pending());
    assert_eq!(host.requests.get(), 6);
    assert!(inv.take_subrequests(inv.remaining_subrequests()));
    host.ready.set(true);
    assert_eq!(check.now_or_never().unwrap().len(), 6);
    host.now.set(host.now.get() + 6000.0);
    let next = Invocation::new(host.clone(), 45);
    assert_eq!(miniapps::check(&next, &pool, &known).now_or_never().unwrap().len(), 12);
    assert_eq!(host.requests.get(), 12);
}

#[test]
fn an_empty_upstream_bucket_does_not_poison_deferred_links_for_an_hour() {
    let host = Rc::new(Pages::new(true));
    let first = Invocation::new(host.clone(), 45);
    let initial = casts("bucket-fill", 0);
    miniapps::check(&first, &initial, &BTreeMap::new()).now_or_never().unwrap();
    let pool = casts("bucket-deferred", 0);
    let busy = Invocation::new(host.clone(), 45);
    assert!(miniapps::check(&busy, &pool, &BTreeMap::new()).now_or_never().unwrap().is_empty());
    assert_eq!(host.requests.get(), 12);
    host.now.set(host.now.get() + 6000.0);
    let next = Invocation::new(host.clone(), 45);
    assert_eq!(miniapps::check(&next, &pool, &BTreeMap::new()).now_or_never().unwrap().len(), 12);
    assert_eq!(host.requests.get(), 24);
}

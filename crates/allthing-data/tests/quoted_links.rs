// SPDX-License-Identifier: AGPL-3.0-or-later
use allthing_core::api::{Annotations, CastPayload, HomePayload, Meta};
use allthing_core::normalize;
use allthing_core::provenance::Backend;
use allthing_core::ranking::TrendingWindow;
use allthing_data::{FetchError, Host, HttpRequest, HttpResponse, Invocation, pages};
use futures::future::{FutureExt, LocalBoxFuture};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

const OUTER: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const INNER: &str = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const LINK: &str = "https://frames.example.test/quote";

#[derive(Default)]
struct Provider {
    requests: RefCell<Vec<String>>,
}

fn outer() -> serde_json::Value {
    serde_json::json!({"hash":OUTER,"author":{"fid":1,"username":"one"},"text":"Ordinary outer cast",
        "timestamp":"2026-10-18T10:00:00Z","embeds":[{"cast_id":{"hash":INNER,"fid":2}}]})
}
fn inner() -> serde_json::Value {
    serde_json::json!({"hash":INNER,"author":{"fid":2,"username":"two"},"text":"A frame on its own domain",
        "timestamp":"2026-10-18T09:00:00Z","embeds":[{"url":LINK}]})
}
impl Host for Provider {
    fn now(&self) -> f64 {
        1_800_000_000_000.0
    }
    fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
        let url = request.url;
        self.requests.borrow_mut().push(url.clone());
        let body = if url.contains("/feed/trending?") {
            Some(serde_json::json!({"casts":[outer()]}).to_string())
        } else if url.contains("/cast/conversation?") {
            Some(serde_json::json!({"conversation":{"cast":outer()}}).to_string())
        } else if url.contains("/casts?") {
            Some(serde_json::json!({"casts":[inner()]}).to_string())
        } else if url.contains("/cast?") {
            Some(serde_json::json!({"cast":inner()}).to_string())
        } else if url == LINK {
            Some("<html><head><meta name='fc:frame' content='vNext'></head></html>".into())
        } else {
            None
        };
        let response = HttpResponse {
            status: if body.is_some() { 200 } else { 404 },
            retry_after: None,
            body: body.map(String::into_bytes),
        };
        async move { Ok(response) }.boxed_local()
    }
}

#[test]
fn quote_metadata_is_checked_in_listings() {
    let host = Rc::new(Provider::default());
    let inv = Rc::new(Invocation::new(host.clone(), 45));
    let result = pages::home(inv, 0, TrendingWindow::Day, None).now_or_never().unwrap().unwrap();
    let payload: HomePayload = serde_json::from_slice(&result.body).unwrap();
    assert_eq!(payload.pool.len(), 1);
    let quoted = &payload.notes.quotes[INNER];
    assert!(quoted.mini_app, "quoted own-domain frame was not detected");
    assert!(quoted.text.is_empty() && quoted.embeds.is_empty());
    assert_eq!(host.requests.borrow().iter().filter(|u| *u == LINK).count(), 1);
}

#[test]
fn quote_metadata_is_checked_on_post_pages() {
    let host = Rc::new(Provider::default());
    let inv = Rc::new(Invocation::new(host.clone(), 45));
    let result = pages::cast(inv, OUTER.into(), None).now_or_never().unwrap().unwrap();
    let payload: CastPayload = serde_json::from_slice(&result.body).unwrap();
    assert_eq!(payload.thread.cast.text, "Ordinary outer cast");
    let quoted = payload.quotes[0].cast.as_ref().unwrap();
    assert!(quoted.mini_app, "quoted own-domain frame was not detected");
    assert!(quoted.text.is_empty() && quoted.embeds.is_empty());
    assert_eq!(host.requests.borrow().iter().filter(|u| *u == LINK).count(), 1);
}

#[test]
fn carried_quote_verdicts_survive_root_checks_without_accumulating_old_links() {
    let host = Rc::new(Provider::default());
    let inv = Rc::new(Invocation::new(host.clone(), 45));
    let quoted = normalize::cast(&serde_json::from_value(inner()).unwrap());
    let previous = HomePayload {
        pool: vec![],
        notes: Annotations {
            quotes: BTreeMap::from([(INNER.to_owned(), quoted.clone())]),
            links: BTreeMap::from([(LINK.to_owned(), false), ("https://obsolete.example/".into(), true)]),
            ..Annotations::default()
        },
        meta: Meta { backend: Backend::Hypersnap, fetched_at: host.now() as i64, stale: false },
    };
    let result = pages::home(inv, 0, TrendingWindow::Day, Some(Rc::new(serde_json::to_vec(&previous).unwrap())))
        .now_or_never()
        .unwrap()
        .unwrap();
    let payload: HomePayload = serde_json::from_slice(&result.body).unwrap();
    assert_eq!(payload.notes.quotes[INNER], quoted);
    assert_eq!(payload.notes.links, BTreeMap::from([(LINK.to_owned(), false)]));
    assert!(!host.requests.borrow().iter().any(|u| u == LINK || u.contains("/casts?")));
}

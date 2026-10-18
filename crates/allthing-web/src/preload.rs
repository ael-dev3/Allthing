// SPDX-License-Identifier: AGPL-3.0-or-later
//! Who liked and recasted the cards in view, loaded before anyone hovers a
//! count, so those popovers open with their people already there.
//!
//! - **Batches follow the list**: a card in view brings the run of
//!   [`ENGAGEMENT_CHUNK`] cards it belongs to (cards 0–7, 8–15, …), in list
//!   order. Every reader of a feed asks for the same batches, so the edge
//!   cache answers most of them and upstream reads don't grow with readers.
//! - **Only where someone reacted**: a zero count opens no popover, so its
//!   list isn't asked for.
//! - **Within a budget** per page view: the cards in view settle for
//!   [`SETTLE_MS`] first, one batch is on its way at a time, batches start
//!   at least [`SPACING_MS`] apart, and a page sends at most
//!   [`MAX_BATCHES`]. That covers the first 24 cards; further cards load
//!   their preview when it opens, as before.
//! - Batches go through [`api::preload_engagement`], with the other
//!   prefetches' cap. Nothing is preloaded on Save-Data or 2g, nor in the
//!   static copy, which has no shared cache.

use std::cell::RefCell;
use std::collections::HashSet;
use std::time::Duration;

use allthing_core::api::{ENGAGEMENT_CHUNK, EngagementRequest};
use leptos::prelude::{TimeoutHandle, set_timeout_with_handle, window};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{Element, IntersectionObserver, IntersectionObserverEntry};

use crate::api::{self, Prefetch};

/// How long the cards in view must stay put before their batch is sent.
const SETTLE_MS: u64 = 200;
/// Batches start at least this far apart.
const SPACING_MS: f64 = 2000.0;
/// Batches one page view sends at most.
const MAX_BATCHES: u32 = 3;
/// When the prefetch slots are taken, look again after this long.
const FULL_RETRY_MS: u64 = 500;

#[derive(Default)]
struct State {
    /// The page (path and query) `sent` and `asked` count for.
    page: String,
    sent: u32,
    /// Batch paths asked for on this page.
    asked: HashSet<String>,
    /// Cards in view, as the observer last reported them.
    in_view: Vec<Element>,
    timer: Option<TimeoutHandle>,
    /// A batch is on its way.
    loading: bool,
    /// When the last batch started, in Unix milliseconds.
    last: f64,
}

thread_local! {
    static OBSERVER: Option<IntersectionObserver> = observer();
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn observer() -> Option<IntersectionObserver> {
    // The static copy builds payloads in the reader's browser, without an
    // edge cache: its popovers load when they open.
    if cfg!(feature = "static-host") {
        return None;
    }
    let callback = Closure::<dyn FnMut(js_sys::Array)>::new(|entries: js_sys::Array| {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            for entry in entries.iter().filter_map(|e| e.dyn_into::<IntersectionObserverEntry>().ok()) {
                let card = entry.target();
                state.in_view.retain(|e| !e.is_same_node(Some(card.as_ref())));
                if entry.is_intersecting() {
                    state.in_view.push(card);
                }
            }
        });
        schedule(SETTLE_MS);
    });
    // Any part of a card in the viewport counts.
    let observer = IntersectionObserver::new(callback.as_ref().unchecked_ref()).ok();
    callback.forget();
    observer
}

/// Preload the previews of `card`'s run while it is in view.
pub fn watch(card: &Element) {
    OBSERVER.with(|o| {
        if let Some(observer) = o {
            observer.observe(card);
        }
    });
}

pub fn unwatch(card: &Element) {
    OBSERVER.with(|o| {
        if let Some(observer) = o {
            observer.unobserve(card);
        }
    });
    STATE.with(|state| state.borrow_mut().in_view.retain(|e| !e.is_same_node(Some(card.as_ref()))));
}

/// Run [`next`] after `ms`, replacing a run already waiting.
fn schedule(ms: u64) {
    let handle = set_timeout_with_handle(next, Duration::from_millis(ms)).ok();
    if let Some(old) = STATE.with(|state| std::mem::replace(&mut state.borrow_mut().timer, handle)) {
        old.clear();
    }
}

enum Step {
    Idle,
    Wait(f64),
    Send(EngagementRequest, String),
}

/// Send the batch of the topmost card in view whose run hasn't been asked
/// for on this page, if the budget allows.
fn next() {
    let page = crate::nav::location_key();
    let now = js_sys::Date::now();
    let step = STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.timer = None;
        if state.page != page {
            state.page = page;
            state.sent = 0;
            state.asked.clear();
        }
        state.in_view.retain(|card| card.is_connected());
        if state.loading || state.sent >= MAX_BATCHES || state.in_view.is_empty() {
            return Step::Idle;
        }
        let wait = state.last + SPACING_MS - now;
        if wait > 0.0 {
            return Step::Wait(wait);
        }
        let height = window().inner_height().ok().and_then(|h| h.as_f64()).unwrap_or(0.0);
        let mut cards: Vec<(f64, &Element)> = state
            .in_view
            .iter()
            .filter_map(|card| {
                let rect = card.get_bounding_client_rect();
                (rect.top() < height && rect.bottom() > 0.0).then_some((rect.top(), card))
            })
            .collect();
        // Top first: the reader reads down.
        cards.sort_by(|a, b| a.0.total_cmp(&b.0));
        cards
            .into_iter()
            .map(|(_, card)| run_of(card))
            .find_map(|request| {
                let path = request.path();
                (!request.is_empty() && !state.asked.contains(&path)).then_some(Step::Send(request, path))
            })
            .unwrap_or(Step::Idle)
    });
    match step {
        Step::Idle => {}
        Step::Wait(ms) => schedule(ms.ceil() as u64),
        Step::Send(request, path) => send(&request, path, now),
    }
}

fn send(request: &EngagementRequest, path: String, now: f64) {
    if api::constrained_network() {
        return;
    }
    let landed = || {
        STATE.with(|state| state.borrow_mut().loading = false);
        // Cards that came into view meanwhile.
        schedule(SETTLE_MS);
    };
    match api::preload_engagement(request, landed) {
        Prefetch::Started => STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.loading = true;
            state.sent += 1;
            state.last = now;
            state.asked.insert(path);
        }),
        // Already loaded on an earlier visit: nothing to send for this run.
        Prefetch::Skipped => {
            STATE.with(|state| state.borrow_mut().asked.insert(path));
            schedule(0);
        }
        Prefetch::Full => schedule(FULL_RETRY_MS),
    }
}

/// The batch for the run of cards `card` is in, in its list's order: each
/// card names its hash and counts in `data-hash`, `data-likes` and
/// `data-recasts`.
fn run_of(card: &Element) -> EngagementRequest {
    let Some(list) = card.parent_element() else { return EngagementRequest::default() };
    let cards = list.children();
    let Some(index) = (0..cards.length()).find(|&i| cards.item(i).is_some_and(|c| c.is_same_node(Some(card.as_ref()))))
    else {
        return EngagementRequest::default();
    };
    let chunk = ENGAGEMENT_CHUNK as u32;
    let start = index - index % chunk;
    let count = |card: &Element, name: &str| card.get_attribute(name).and_then(|n| n.parse().ok()).unwrap_or(0);
    let run: Vec<(String, u64, u64)> = (start..(start + chunk).min(cards.length()))
        .filter_map(|i| cards.item(i))
        .filter_map(|c| Some((c.get_attribute("data-hash")?, count(&c, "data-likes"), count(&c, "data-recasts"))))
        .collect();
    EngagementRequest::for_casts(run.iter().map(|(hash, likes, recasts)| (hash.as_str(), *likes, *recasts)))
}

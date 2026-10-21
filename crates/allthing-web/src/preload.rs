// SPDX-License-Identifier: AGPL-3.0-or-later
//! Who liked and recasted the feed cards in view and just below it, loaded
//! before anyone hovers a count, so those popovers open with their people
//! already there.
//!
//! - **Cards in view**: the shared prefetch observer in `components/cast.rs`
//!   reports cards entering and leaving the viewport. Once they have stayed
//!   put for [`SETTLE_MS`], the topmost card in view whose batch this page
//!   hasn't asked for brings it.
//! - **Then the next run**, ahead of the reader: the run after the last card
//!   in view brings its batch once its first card is less than a screen
//!   below the viewport ([`AHEAD_SCREENS`]) and all its cards are on the
//!   page (a run the next page is still to fill would be asked for twice).
//! - **Batches follow the list**: a card brings the run of
//!   [`ENGAGEMENT_CHUNK`] cards it belongs to, in list order (cards 1–6,
//!   7–12, …), with a list only for counts above zero. Readers of the same
//!   feed ask for the same batches, so the edge cache answers most of them
//!   and upstream reads don't grow with readers.
//! - **Within a budget** per page view: one batch on its way at a time,
//!   batches at least [`SPACING_MS`] apart, and at most [`MAX_BATCHES`]. One
//!   reader's batches ask Hypersnap for at most 6 reads a second, below the
//!   10 a second an isolate's bucket refills (docs/PERFORMANCE.md). Later
//!   cards load their previews when they open, as before.
//! - Batches go through [`api::preload_engagement`], under the prefetch cap.
//!   They load on touch screens too, so a count tapped open is as complete
//!   as one hovered. Nothing is preloaded on Save-Data or 2g, nor in the
//!   static copy, whose cards aren't observed.

use std::cell::RefCell;
use std::collections::HashSet;
use std::time::Duration;

use allthing_core::api::{ENGAGEMENT_CHUNK, EngagementRequest};
use leptos::prelude::{TimeoutHandle, set_timeout_with_handle, window};
use web_sys::Element;

use crate::api::{self, Prefetch};

/// How long the cards in view must stay put before their batch is sent.
const SETTLE_MS: u64 = 200;
/// Batches start at least this far apart.
const SPACING_MS: f64 = 2000.0;
/// Batches one page view sends at most: the first 48 cards, about the
/// front page's first two pages.
const MAX_BATCHES: u32 = 8;
/// The next run's batch is sent once its first card is no more than this
/// many screens below the viewport.
const AHEAD_SCREENS: f64 = 1.0;
/// When the prefetch slots are taken, look again after this long.
const FULL_RETRY_MS: u64 = 500;

#[derive(Default)]
struct State {
    /// The page (path and query) that `sent` and `asked` count for.
    page: String,
    /// Batches sent for this page.
    sent: u32,
    /// Batch paths this page has asked for.
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
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// A card came into view (`in_view`) or left it, as the prefetch observer
/// reports it.
pub fn noticed(card: Element, in_view: bool) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.in_view.retain(|seen| *seen != card);
        if in_view {
            state.in_view.push(card);
        }
    });
    schedule(SETTLE_MS);
}

/// A card left the page.
pub fn forget(card: &Element) {
    STATE.with(|state| state.borrow_mut().in_view.retain(|seen| seen != card));
}

/// A list was rendered anew, as on a new page or after Back: it is a new
/// page view, with its own [`MAX_BATCHES`].
pub fn new_view() {
    STATE.with(|state| state.borrow_mut().page.clear());
}

/// Run [`next`] after `ms`, in place of a run already waiting.
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

/// Send the batch of the topmost card in view whose run this page hasn't
/// asked for, or else of the next run once it is near, if the budget
/// allows.
fn next() {
    STATE.with(|state| state.borrow_mut().timer = None);
    if api::constrained_network() {
        return;
    }
    let page = crate::nav::location_key();
    let now = js_sys::Date::now();
    let step = STATE.with(|state| {
        let mut state = state.borrow_mut();
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
        let ahead = cards.last().and_then(|(_, card)| next_run(card, height * (1.0 + AHEAD_SCREENS)));
        cards
            .into_iter()
            .map(|(_, card)| run_of(card))
            .chain(ahead)
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
        // Already kept from an earlier visit, or on its way: nothing to
        // send for this run, so look at the next one.
        Prefetch::Skipped => {
            STATE.with(|state| state.borrow_mut().asked.insert(path));
            schedule(0);
        }
        Prefetch::Full => schedule(FULL_RETRY_MS),
    }
}

/// The cards of `card`'s list, in order. Each names its hash and counts in
/// `data-hash`, `data-likes` and `data-recasts`.
fn cards_beside(card: &Element) -> Vec<Element> {
    let mut cards = Vec::new();
    let mut sibling = card.parent_element().and_then(|list| list.first_element_child());
    while let Some(element) = sibling {
        sibling = element.next_element_sibling();
        if element.has_attribute("data-hash") {
            cards.push(element);
        }
    }
    cards
}

/// The batch for the run of cards that `card` is in, in its list's order.
fn run_of(card: &Element) -> EngagementRequest {
    let cards = cards_beside(card);
    let Some(index) = cards.iter().position(|c| c == card) else { return EngagementRequest::default() };
    let start = index - index % ENGAGEMENT_CHUNK;
    request_for(&cards[start..cards.len().min(start + ENGAGEMENT_CHUNK)])
}

/// The batch for the run after the one `card` is in, once that run's first
/// card is less than `below` pixels from the viewport's top and all its
/// cards are on the page.
fn next_run(card: &Element, below: f64) -> Option<EngagementRequest> {
    let cards = cards_beside(card);
    let index = cards.iter().position(|c| c == card)?;
    let start = index - index % ENGAGEMENT_CHUNK + ENGAGEMENT_CHUNK;
    let run = cards.get(start..start + ENGAGEMENT_CHUNK)?;
    (run[0].get_bounding_client_rect().top() < below).then(|| request_for(run))
}

/// Who liked and recasted the cards of `run` with counts above zero.
fn request_for(run: &[Element]) -> EngagementRequest {
    let count = |card: &Element, name: &str| card.get_attribute(name).and_then(|n| n.parse().ok()).unwrap_or(0);
    let run: Vec<(String, u64, u64)> = run
        .iter()
        .filter_map(|c| Some((c.get_attribute("data-hash")?, count(c, "data-likes"), count(c, "data-recasts"))))
        .collect();
    EngagementRequest::for_casts(run.iter().map(|(hash, likes, recasts)| (hash.as_str(), *likes, *recasts)))
}

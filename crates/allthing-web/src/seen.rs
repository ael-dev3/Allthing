// SPDX-License-Identifier: AGPL-3.0-or-later
//! Seen casts on the front page (`allthing_core::seen`). A card of the front
//! page that is at least half in view (or fills half the window) for a
//! second is recorded as seen, in this browser's localStorage
//! (`allthing.seen`, the newest 5,000 hashes), signed in or out; the server
//! keeps nothing. The front page marks the casts seen before this visit
//! (read once, as the app starts, so nothing moves while the reader is
//! here): dimmed, with "Seen" in their meta row, and, in hot, listed after
//! the casts not seen yet, page by page. Nothing is left out. web/paint.js
//! draws the first cards the same way.
//!
//! The reader can turn it off (`allthing.seen-marks` = `"0"`): then nothing
//! is marked, moved or recorded. Their other tabs follow the choice.

use std::cell::RefCell;
use std::collections::HashSet;
use std::time::Duration;

use allthing_core::model::Cast;
use allthing_core::ranking::Sort;
use allthing_core::seen::{CAP, SeenSet, unseen_first};
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{IntersectionObserver, IntersectionObserverEntry, IntersectionObserverInit};

const KEY: &str = "allthing.seen";
const SWITCH_KEY: &str = "allthing.seen-marks";
/// How long a card stays in view to count as seen.
const DWELL: Duration = Duration::from_millis(1000);
/// How long new sightings wait to be written, together.
const FLUSH: Duration = Duration::from_millis(2000);

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

fn stored() -> SeenSet {
    let text = storage().and_then(|s| s.get_item(KEY).ok().flatten()).unwrap_or_default();
    SeenSet::parse(&text, CAP)
}

fn read_on() -> bool {
    storage().and_then(|s| s.get_item(SWITCH_KEY).ok().flatten()).is_none_or(|v| v != "0")
}

thread_local! {
    /// What was seen before this visit.
    static BEFORE: SeenSet = stored();
    /// Seen this visit, recorded or waiting to be.
    static RECORDED: RefCell<HashSet<String>> = RefCell::default();
    /// Seen, not written yet.
    static PENDING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Cards waiting out their second in view.
    static DWELLING: RefCell<HashSet<String>> = RefCell::default();
    static OBSERVER: Option<IntersectionObserver> = observer();
    static ON: ArcRwSignal<bool> = {
        let on = ArcRwSignal::new(read_on());
        let follow = on.clone();
        let _ = window_event_listener(leptos::ev::storage, move |ev| {
            if ev.key().is_none_or(|key| key == SWITCH_KEY) {
                follow.set(read_on());
            }
        });
        // What's waiting is written as the page goes away.
        let _ = window_event_listener(leptos::ev::pagehide, |_| flush());
        on
    };
}

/// Whether seen casts are marked (tracked).
pub fn enabled() -> bool {
    ON.with(|on| on.get())
}

fn enabled_untracked() -> bool {
    ON.with(|on| on.get_untracked())
}

/// Mark seen casts or not, here and in the reader's other tabs.
// The static copy has no account menu to hold the switch.
#[cfg_attr(feature = "static-host", allow(dead_code))]
pub fn set_enabled(on: bool) {
    ON.with(|s| s.set(on));
    if let Some(storage) = storage() {
        let _ = if on { storage.remove_item(SWITCH_KEY) } else { storage.set_item(SWITCH_KEY, "0") };
    }
}

/// The front page's cards record and show being seen: it provides this.
#[derive(Clone, Copy)]
pub struct Marks;

pub fn provide_marks() {
    provide_context(Marks);
}

/// Whether the cards drawn here are the front page's.
pub fn on_front() -> bool {
    use_context::<Marks>().is_some()
}

/// Whether `hash` was seen before this visit.
pub fn seen_before(hash: &str) -> bool {
    BEFORE.with(|b| b.contains(hash))
}

/// In hot, the casts not seen before this visit first (tracked: the switch).
pub fn hot_order(casts: Vec<Cast>, order: Sort) -> Vec<Cast> {
    if order != Sort::Hot || !enabled() {
        return casts;
    }
    unseen_first(casts, |cast| seen_before(&cast.hash))
}

/// Watch a front page card for being seen.
pub fn watch(element: &web_sys::Element) {
    OBSERVER.with(|o| {
        if let Some(observer) = o {
            observer.observe(element);
        }
    });
}

pub fn unwatch(element: &web_sys::Element) {
    OBSERVER.with(|o| {
        if let Some(observer) = o {
            observer.unobserve(element);
        }
    });
}

fn observer() -> Option<IntersectionObserver> {
    let callback = Closure::<dyn FnMut(js_sys::Array)>::new(|entries: js_sys::Array| {
        if !enabled_untracked() {
            return;
        }
        for entry in entries.iter().filter_map(|e| e.dyn_into::<IntersectionObserverEntry>().ok()) {
            if !entry.is_intersecting() {
                continue;
            }
            let target = entry.target();
            let Some(hash) = target.get_attribute("data-hash") else { continue };
            if RECORDED.with(|r| r.borrow().contains(&hash)) || !mostly_visible(&target) {
                continue;
            }
            if !DWELLING.with(|d| d.borrow_mut().insert(hash.clone())) {
                continue;
            }
            set_timeout(
                move || {
                    DWELLING.with(|d| d.borrow_mut().remove(&hash));
                    if target.is_connected() && mostly_visible(&target) && enabled_untracked() {
                        record(hash);
                    }
                },
                DWELL,
            );
        }
    });
    let options = IntersectionObserverInit::new();
    let steps = js_sys::Array::new();
    for i in 0..=10 {
        steps.push(&JsValue::from_f64(f64::from(i) / 10.0));
    }
    options.set_threshold(&steps);
    let observer = IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options).ok();
    callback.forget();
    observer
}

/// At least half of it in view, or, for one taller than that, half the window.
fn mostly_visible(element: &web_sys::Element) -> bool {
    let rect = element.get_bounding_client_rect();
    let height = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    let shown = rect.bottom().min(height) - rect.top().max(0.0);
    shown > 0.0 && (shown >= rect.height() * 0.5 || shown >= height * 0.5)
}

fn record(hash: String) {
    if !RECORDED.with(|r| r.borrow_mut().insert(hash.clone())) {
        return;
    }
    let first = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        p.push(hash);
        p.len() == 1
    });
    if first {
        set_timeout(flush, FLUSH);
    }
}

/// Write what's waiting, on top of what the browser has (another tab's too).
fn flush() {
    let pending = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
    if pending.is_empty() {
        return;
    }
    let mut seen = stored();
    for hash in &pending {
        seen.insert(hash);
    }
    if let Some(storage) = storage() {
        let _ = storage.set_item(KEY, &seen.to_stored());
    }
}

/// The switch in the account menu: "Mark seen casts", drawn as the mini
/// app one is.
#[cfg_attr(feature = "static-host", allow(dead_code))]
#[component]
pub fn SeenSwitch() -> impl IntoView {
    let on = Memo::new(move |_| enabled());
    view! {
        <button
            type="button"
            role="switch"
            class="mini-apps-switch seen-switch account-link"
            aria-checked=move || on.get().to_string()
            aria-description="Dims the front page's casts you've seen before, and lists them lower in hot. Kept in this browser."
            on:click=move |_| set_enabled(!on.get_untracked())
        >
            <span class="switch-track" aria-hidden="true"><span class="switch-thumb"></span></span>
            "Mark seen casts"
        </button>
    }
}

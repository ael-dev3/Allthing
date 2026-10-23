// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local reading history: foreground viewport dwell, never fetched payloads.
//! Feed snapshots only change on a new query/load/account or explicit reset.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use allthing_core::seen::History;
use leptos::prelude::*;
use leptos_router::hooks::use_url;
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{Element, IntersectionObserver, IntersectionObserverEntry, IntersectionObserverInit};

const KEY: &str = "allthing.seen.v1";
const DWELL_MS: f64 = 1_000.0;

#[derive(Clone)]
struct Pending {
    at: i64,
    revision: u64,
}

#[derive(Default)]
struct State {
    history: History,
    pending: BTreeMap<(String, String), Pending>,
    flush: Option<TimeoutHandle>,
    storage_failed: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static WATCHED: RefCell<Vec<Watch>> = const { RefCell::new(Vec::new()) };
    static WAKE_QUEUED: Cell<bool> = const { Cell::new(false) };
    static OBSERVER: Option<IntersectionObserver> = observer();
}

fn now() -> i64 {
    (js_sys::Date::now() / 1_000.0) as i64
}

fn storage() -> Result<web_sys::Storage, ()> {
    window().local_storage().ok().flatten().ok_or(())
}

fn read() -> Result<History, ()> {
    let text = storage()?.get_item(KEY).map_err(|_| ())?;
    Ok(text.map_or_else(History::default, |text| History::read(&text, now())))
}

fn reload(state: &mut State) {
    if !state.storage_failed {
        match read() {
            Ok(history) => state.history = history,
            Err(()) => state.storage_failed = true,
        }
    }
    let now = now();
    state.pending.retain(|(scope, _), pending| pending.revision == state.history.revision(scope));
    for ((scope, hash), pending) in &state.pending {
        state.history.record_at(scope, hash, pending.at, now);
    }
}

fn persist(state: &mut State) {
    let result = storage().and_then(|storage| {
        if state.history.is_empty() { storage.remove_item(KEY) } else { storage.set_item(KEY, &state.history.encode()) }
            .map_err(|_| ())
    });
    if result.is_err() {
        state.storage_failed = true;
    }
}

fn flush() {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.flush = None;
        // Merge only this tab's newly seen hashes with current storage, so
        // another tab's reset is never undone by our older full snapshot.
        reload(&mut state);
        persist(&mut state);
        state.pending.clear();
    });
}

fn record(scope: &str, hash: &str) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let at = now();
        state.history.record(scope, hash, at);
        let revision = state.history.revision(scope);
        state.pending.insert((scope.into(), hash.into()), Pending { at, revision });
        if state.flush.is_none() {
            state.flush = set_timeout_with_handle(flush, Duration::from_millis(500)).ok();
        }
    });
}

/// Shared account and reset generation. Reading a cast never changes this
/// signal: cards on screen keep their place while the history is recorded.
#[derive(Clone, Copy)]
pub struct Seen {
    scope: Memo<String>,
    epoch: RwSignal<u32>,
}

impl Seen {
    pub fn new() -> Self {
        #[cfg(not(feature = "static-host"))]
        let scope = {
            let session = crate::components::signin::use_session();
            Memo::new(move |_| allthing_core::seen::scope(session.user.with(|u| u.as_ref().map(|u| u.fid))))
        };
        #[cfg(feature = "static-host")]
        let scope = Memo::new(move |_| allthing_core::seen::scope(None));
        let seen = Self { scope, epoch: RwSignal::new(0) };
        let _ = window_event_listener(leptos::ev::storage, move |event| {
            if event.key().is_none_or(|key| key == KEY) {
                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    state.storage_failed = false;
                    if event.new_value().is_none() {
                        state.pending.clear();
                    }
                    reload(&mut state);
                });
            }
        });
        let _ = window_event_listener_untyped("pagehide", move |_| flush());
        let visibility = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| wake());
        let _ = crate::util::document()
            .add_event_listener_with_callback("visibilitychange", visibility.as_ref().unchecked_ref());
        visibility.forget();
        // Tall cards may never cross an observer ratio of 0.5. Recheck only
        // intersecting cards on scroll/resize, at most once per paint.
        let _ = window_event_listener_untyped("scroll", move |_| queue_wake());
        let _ = window_event_listener_untyped("resize", move |_| queue_wake());
        seen
    }

    fn reset(self) {
        let scope = self.scope.get_untracked();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.pending.retain(|(name, _), _| name != &scope);
            reload(&mut state);
            state.history.reset(&scope, now());
            persist(&mut state);
        });
        self.epoch.update(|epoch| *epoch = epoch.wrapping_add(1));
    }
}

pub fn use_seen() -> Seen {
    expect_context()
}

/// Settings' reversible reset affects the current reader only, in this browser.
#[component]
pub fn ResetSeen() -> impl IntoView {
    let seen = use_seen();
    let cleared = RwSignal::new(false);
    view! {
        <button type="button" class="account-link" on:click=move |_| { seen.reset(); cleared.set(true); }>"Reset seen posts"</button>
        <p class="muted small" role="status">{move || if cleared.get() { "Seen history reset for this reader." } else { "Reading history stays in this browser for 30 days." }}</p>
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Snapshot {
    key: (String, u32, String),
    hashes: BTreeSet<String>,
}

#[derive(Clone, Copy)]
pub struct Feed {
    snapshot: Memo<Snapshot>,
    shown: RwSignal<Option<(String, u32, String)>>,
}

impl Feed {
    pub fn new(enabled: bool) -> Self {
        let seen = use_seen();
        let url = use_url();
        // A query selects a new feed generation. As with Home's ranking,
        // wait for the selected tab to paint before filtering its cards:
        // suppressing just-read rows can dispose most of the old list when
        // progressive drawing restarts, which must not block the tap.
        let query = RwSignal::new(url.with_untracked(|url| url.search().to_owned()));
        Effect::new(move |_| {
            if !enabled {
                return;
            }
            let current = url.get();
            if crate::kept::is_front(current.path()) && current.search() != query.get_untracked() {
                crate::components::cast::after_paint(move || {
                    let Some(current) = url.try_get_untracked() else { return };
                    if crate::kept::is_front(current.path()) {
                        let _ = query.try_set(current.search().to_owned());
                    }
                });
            }
        });
        let previous = StoredValue::new(Snapshot::default());
        let snapshot = Memo::new(move |_| {
            if !enabled {
                return Snapshot::default();
            }
            let before = previous.get_value();
            // Home is kept, detached, while reading a post/profile. Preserve
            // that generation for Back, rather than shortening its scroll.
            let key = (seen.scope.get(), seen.epoch.get(), query.get());
            if key == before.key {
                return before;
            }
            let hashes = STATE.with(|state| {
                let mut state = state.borrow_mut();
                reload(&mut state);
                state.history.hashes(&key.0)
            });
            let snapshot = Snapshot { key, hashes };
            previous.set_value(snapshot.clone());
            snapshot
        });
        Self { snapshot, shown: RwSignal::new(None) }
    }

    pub fn hides(self, hash: &str) -> bool {
        self.snapshot.with(|snapshot| {
            self.shown.get().as_ref() != Some(&snapshot.key) && snapshot.hashes.contains(&hash.to_ascii_lowercase())
        })
    }
}

/// Below the list, so a later page's result cannot move existing cards.
#[component]
pub fn SeenNotice(feed: Feed, #[prop(into)] hidden: Signal<usize>, #[prop(into)] empty: Signal<bool>) -> impl IntoView {
    let seen = use_seen();
    view! {
        <Show when=move || { hidden.get() > 0 }>
            <div class="seen-notice">
                <p>{move || if empty.get() { "You're caught up in this feed. Previously seen posts are hidden." } else { "Previously seen posts are hidden." }}</p>
                <button type="button" class="account-link" on:click=move |_| feed.shown.set(Some(feed.snapshot.with_untracked(|s| s.key.clone())))>"Show seen posts"</button>
                <button type="button" class="account-link" on:click=move |_| seen.reset()>"Reset seen posts"</button>
            </div>
        </Show>
    }
}

struct Watch {
    element: Element,
    seen: Seen,
    scope: String,
    epoch: u32,
    started: Option<f64>,
    timer: Option<TimeoutHandle>,
    done: bool,
    intersecting: bool,
}

fn eligible(element: &Element) -> bool {
    let document = crate::util::document();
    if document.hidden()
        || document.get_element_by_id("boot").is_some()
        || !element.is_connected()
        || element.closest("[inert]").ok().flatten().is_some()
    {
        return false;
    }
    let rect = element.get_bounding_client_rect();
    let height = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    let width = window().inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    // A long cast can be taller than the viewport. Half of what could
    // fit on screen counts, rather than requiring an impossible half of it.
    let area = rect.width().min(width) * rect.height().min(height);
    let visible = (rect.bottom().min(height) - rect.top().max(0.0)).max(0.0)
        * (rect.right().min(width) - rect.left().max(0.0)).max(0.0);
    area > 0.0 && visible / area >= 0.5
}

fn stop(watch: &mut Watch) {
    if let Some(timer) = watch.timer.take() {
        timer.clear();
    }
    watch.started = None;
}

fn notice(element: &Element) {
    WATCHED.with(|watched| {
        let mut watched = watched.borrow_mut();
        let Some(watch) = watched.iter_mut().find(|w| w.element == *element) else { return };
        if watch.done {
            return;
        }
        if !eligible(element)
            || watch.seen.scope.get_untracked() != watch.scope
            || watch.seen.epoch.get_untracked() != watch.epoch
        {
            stop(watch);
            return;
        }
        if watch.started.is_none() {
            watch.started = Some(js_sys::Date::now());
            let target = element.clone();
            watch.timer = set_timeout_with_handle(move || finish(&target), Duration::from_millis(DWELL_MS as u64)).ok();
        }
    });
}

fn finish(element: &Element) {
    let record = WATCHED.with(|watched| {
        let mut watched = watched.borrow_mut();
        let watch = watched.iter_mut().find(|w| w.element == *element)?;
        watch.timer = None;
        let stayed = watch.started.take().is_some_and(|at| js_sys::Date::now() - at >= DWELL_MS);
        if !stayed
            || !eligible(element)
            || watch.seen.scope.get_untracked() != watch.scope
            || watch.seen.epoch.get_untracked() != watch.epoch
        {
            return None;
        }
        watch.done = true;
        Some((watch.scope.clone(), element.get_attribute("data-hash")?))
    });
    if let Some((scope, hash)) = record {
        self::record(&scope, &hash);
    }
}

fn observer() -> Option<IntersectionObserver> {
    let callback = Closure::<dyn FnMut(js_sys::Array)>::new(|entries: js_sys::Array| {
        for entry in entries.iter().filter_map(|e| e.dyn_into::<IntersectionObserverEntry>().ok()) {
            WATCHED.with(|watched| {
                if let Some(watch) = watched.borrow_mut().iter_mut().find(|w| w.element == entry.target()) {
                    watch.intersecting = entry.is_intersecting();
                }
            });
            notice(&entry.target());
        }
    });
    let options = IntersectionObserverInit::new();
    let thresholds = js_sys::Array::new();
    for ratio in [0.0, 0.5, 1.0] {
        thresholds.push(&ratio.into());
    }
    options.set_threshold(&thresholds);
    let observer = IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options).ok();
    callback.forget();
    observer
}

/// Only CastCards belonging to an opted-in front-page list are observed.
/// Re-registering after sign-in/reset cancels the old account's timer.
pub fn watch(element: &Element, seen: Seen) {
    unwatch(element);
    let scope = seen.scope.get();
    let epoch = seen.epoch.get();
    WATCHED.with(|watched| {
        watched.borrow_mut().push(Watch {
            element: element.clone(),
            seen,
            scope,
            epoch,
            started: None,
            timer: None,
            done: false,
            intersecting: false,
        })
    });
    OBSERVER.with(|observer| {
        if let Some(observer) = observer {
            observer.observe(element);
        }
    });
}

pub fn unwatch(element: &Element) {
    OBSERVER.with(|observer| {
        if let Some(observer) = observer {
            observer.unobserve(element);
        }
    });
    WATCHED.with(|watched| {
        watched.borrow_mut().retain_mut(|watch| {
            if watch.element != *element {
                return true;
            }
            stop(watch);
            false
        })
    });
}

/// The boot screen yielded to the app, or document visibility changed.
pub fn wake() {
    let elements = WATCHED.with(|watched| {
        watched.borrow().iter().filter(|w| w.intersecting).map(|w| w.element.clone()).collect::<Vec<_>>()
    });
    for element in elements {
        notice(&element);
    }
}

fn queue_wake() {
    if WAKE_QUEUED.with(|queued| queued.replace(true)) {
        return;
    }
    request_animation_frame(move || {
        WAKE_QUEUED.with(|queued| queued.set(false));
        wake();
    });
}

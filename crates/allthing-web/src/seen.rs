// SPDX-License-Identifier: AGPL-3.0-or-later
//! Foreground viewport dwell, never fetched payloads. Signed-in history is
//! authoritative in the private Library; anonymous readers keep a local cache.
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

/// Import only the current FID's old browser cache after private metadata has
/// arrived. The Library fences the migration against remote clears/tombstones.
#[cfg(not(feature = "static-host"))]
pub(crate) fn legacy_entries(fid: u64) -> Vec<(String, i64)> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        reload(&mut state);
        state.history.entries(&allthing_core::seen::scope(Some(fid)))
    })
}

/// Shared account and reset generation. Reading a cast never changes this
/// signal: cards on screen keep their place while the history is recorded.
#[derive(Clone, Copy)]
pub struct Seen {
    scope: Memo<String>,
    epoch: RwSignal<u32>,
    generation: Memo<u64>,
    #[cfg(not(feature = "static-host"))]
    library: crate::library::Library,
}

impl Seen {
    /// A per-card memo keeps unrelated Library updates out of the dwell
    /// registration effect, but lets an explicit Mark unread start a new read.
    pub fn read_mark(self, hash: String) -> Memo<Option<bool>> {
        #[cfg(not(feature = "static-host"))]
        {
            Memo::new(move |_| self.library.seen(&hash))
        }
        #[cfg(feature = "static-host")]
        {
            let _ = hash;
            Memo::new(move |_| None)
        }
    }

    fn generation(self) -> u64 {
        self.generation.get()
    }

    pub fn new() -> Self {
        #[cfg(not(feature = "static-host"))]
        let scope = {
            let session = crate::components::signin::use_session();
            Memo::new(move |_| allthing_core::seen::scope(session.user.with(|u| u.as_ref().map(|u| u.fid))))
        };
        #[cfg(feature = "static-host")]
        let scope = Memo::new(move |_| allthing_core::seen::scope(None));
        #[cfg(not(feature = "static-host"))]
        let library = crate::library::use_library();
        #[cfg(not(feature = "static-host"))]
        let generation = Memo::new(move |_| library.generation());
        #[cfg(feature = "static-host")]
        let generation = Memo::new(move |_| 0);
        let seen = Self {
            scope,
            epoch: RwSignal::new(0),
            generation,
            #[cfg(not(feature = "static-host"))]
            library,
        };
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

/// Signed-in history controls are in Library, where scope and consequences are
/// explicit. Anonymous/legacy readers can still reset their local cache here.
#[component]
pub fn ResetSeen() -> impl IntoView {
    let seen = use_seen();
    let cleared = RwSignal::new(false);
    let remote = move || {
        #[cfg(not(feature = "static-host"))]
        {
            seen.library.remote_required()
        }
        #[cfg(feature = "static-host")]
        {
            false
        }
    };
    view! {
        <Show when=remote fallback=move || view! {
            <button type="button" class="account-link" on:click=move |_| { seen.reset(); cleared.set(true); }>"Reset seen posts"</button>
            <p class="muted small" role="status">{move || if cleared.get() { "Seen history reset for this reader." } else { "This reader's local history stays in this browser for 30 days. Sign in on allthing.social to sync durable history." }}</p>
        }>
            <a class="account-link" href=crate::util::href("/library?tab=seen")>"Manage seen posts"</a>
            <p class="muted small">"Seen posts stay hidden on future Home visits across your signed-in devices. Manage or clear your private history in Library."</p>
        </Show>
    }
}

type FeedKey = (String, u32, String, bool, u64);

#[derive(Clone, Default, PartialEq, Eq)]
struct Snapshot {
    key: FeedKey,
    hashes: BTreeSet<String>,
}

#[derive(Default)]
struct Pins {
    key: FeedKey,
    drawn: BTreeSet<String>,
    hidden: BTreeSet<String>,
}

impl Pins {
    fn select(&mut self, key: &FeedKey) {
        if &self.key != key {
            self.key = key.clone();
            self.drawn.clear();
            self.hidden.clear();
        }
    }
}

#[derive(Clone, Copy)]
pub struct Feed {
    snapshot: Memo<Snapshot>,
    shown: RwSignal<Option<FeedKey>>,
    pinned: StoredValue<Pins>,
    candidates: Signal<Vec<String>>,
    enabled: bool,
    #[cfg(not(feature = "static-host"))]
    library: crate::library::Library,
}

impl Feed {
    pub fn new(enabled: bool, candidates: Signal<Vec<String>>) -> Self {
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
        #[cfg(not(feature = "static-host"))]
        let library = seen.library;
        #[cfg(not(feature = "static-host"))]
        if enabled {
            let owner = format!("feed:{}", js_sys::Math::random());
            let watched = owner.clone();
            Effect::new(move |_| {
                let hashes = candidates.get();
                untrack(|| library.watch_batch(&watched, &hashes));
            });
            on_cleanup(move || library.unwatch_batch(&owner));
        }
        let snapshot = Memo::new(move |_| {
            if !enabled {
                return Snapshot::default();
            }
            let before = previous.get_value();
            // Home is kept, detached, while reading a post/profile. Preserve
            // that generation for Back, rather than shortening its scroll.
            #[cfg(not(feature = "static-host"))]
            let (remote, generation) = (library.remote_required(), library.generation());
            #[cfg(feature = "static-host")]
            let (remote, generation) = (false, 0);
            let key = (seen.scope.get(), seen.epoch.get(), query.get(), remote, generation);
            if key == before.key {
                return before;
            }
            let hashes = if remote {
                BTreeSet::new()
            } else {
                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    reload(&mut state);
                    state.history.hashes(&key.0)
                })
            };
            let snapshot = Snapshot { key, hashes };
            previous.set_value(snapshot.clone());
            snapshot
        });
        Self {
            snapshot,
            shown: RwSignal::new(None),
            pinned: StoredValue::new(Pins::default()),
            candidates,
            enabled,
            #[cfg(not(feature = "static-host"))]
            library,
        }
    }

    pub fn hides(self, hash: &str) -> bool {
        if !self.enabled {
            return false;
        }
        self.snapshot.with(|snapshot| {
            #[cfg(not(feature = "static-host"))]
            if snapshot.key.3 {
                if self.pinned.with_value(|pins| pins.key == snapshot.key && pins.drawn.contains(hash)) {
                    return false;
                }
                if self.pinned.with_value(|pins| pins.key == snapshot.key && pins.hidden.contains(hash)) {
                    return true;
                }
                if self.library.seen(hash) == Some(true) {
                    if self.library.membership_ready(&[hash.to_owned()]) {
                        self.pinned.update_value(|pins| {
                            pins.select(&snapshot.key);
                            pins.hidden.insert(hash.to_owned());
                        });
                    }
                    return true;
                }
                return !self.library.membership_ready(&[hash.to_owned()]) || self.library.seen(hash) != Some(false);
            }
            self.shown.get().as_ref() != Some(&snapshot.key) && snapshot.hashes.contains(&hash.to_ascii_lowercase())
        })
    }

    /// Freeze only cards actually drawn, so reading one never moves the page.
    /// Candidates below the drawn window still honor newly synced history.
    pub fn preserve(self, hash: &str) {
        let snapshot = self.snapshot.get_untracked();
        self.pinned.update_value(|pins| {
            pins.select(&snapshot.key);
            pins.drawn.insert(hash.to_owned());
        });
    }

    pub fn pending(self) -> bool {
        #[cfg(not(feature = "static-host"))]
        {
            self.enabled && self.library.remote_required() && !self.library.membership_ready(&self.candidates.get())
        }
        #[cfg(feature = "static-host")]
        {
            let _ = self.candidates;
            false
        }
    }

    pub fn hidden(self, hash: &str) -> bool {
        #[cfg(not(feature = "static-host"))]
        if self.enabled && self.library.remote_required() {
            return self.hides(hash)
                && (self.library.seen(hash) == Some(true)
                    || self.pinned.with_value(|pins| {
                        pins.key == self.snapshot.with_untracked(|s| s.key.clone()) && pins.hidden.contains(hash)
                    }));
        }
        self.hides(hash)
    }
}

/// Below the list, so a later page's result cannot move existing cards.
#[component]
pub fn SeenNotice(feed: Feed, #[prop(into)] hidden: Signal<usize>, #[prop(into)] empty: Signal<bool>) -> impl IntoView {
    let seen = use_seen();
    view! {
        <Show when=move || feed.pending()>
            <SyncNotice/>
        </Show>
        <Show when=move || { hidden.get() > 0 }>
            <div class="seen-notice">
                <p>{move || if empty.get() { "You're caught up in this feed. Previously seen posts are hidden." } else { "Previously seen posts are hidden." }}</p>
                <Show when=move || !feed.snapshot.with(|s| s.key.3) fallback=|| view! { <a class="account-link" href=crate::util::href("/library?tab=seen")>"View seen posts in Library"</a> }>
                    <button type="button" class="account-link" on:click=move |_| feed.shown.set(Some(feed.snapshot.with_untracked(|s| s.key.clone())))>"Show seen posts"</button>
                    <button type="button" class="account-link" on:click=move |_| seen.reset()>"Reset seen posts"</button>
                </Show>
            </div>
        </Show>
    }
}

#[component]
fn SyncNotice() -> impl IntoView {
    #[cfg(not(feature = "static-host"))]
    {
        let library = crate::library::use_library();
        view! {
            <div class="seen-notice" role="status">
                <p>{move || match library.status() {
                    crate::library::SyncStatus::Failed(error) => format!("Couldn't sync reading history: {error}"),
                    crate::library::SyncStatus::Offline => "You're offline. New Home posts will wait until your reading history can sync.".to_owned(),
                    _ => "Syncing reading history before showing new posts…".to_owned(),
                }}</p>
                <button type="button" class="account-link" on:click=move |_| library.retry()>"Retry history sync"</button>
                <a class="account-link" href=crate::util::href("/library?tab=seen")>"Open Library"</a>
            </div>
        }.into_any()
    }
    #[cfg(feature = "static-host")]
    {
        ().into_any()
    }
}

struct Watch {
    element: Element,
    seen: Seen,
    scope: String,
    epoch: u32,
    generation: u64,
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
            || untrack(|| watch.seen.generation()) != watch.generation
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
            || untrack(|| watch.seen.generation()) != watch.generation
        {
            return None;
        }
        watch.done = true;
        Some((watch.seen, watch.scope.clone(), element.get_attribute("data-hash")?))
    });
    if let Some((seen, scope, hash)) = record {
        self::record(&scope, &hash);
        #[cfg(not(feature = "static-host"))]
        if seen.library.remote_required() {
            let _ = seen.library.record_seen(&hash, now());
        }
        #[cfg(feature = "static-host")]
        let _ = seen;
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
    let generation = seen.generation();
    WATCHED.with(|watched| {
        watched.borrow_mut().push(Watch {
            element: element.clone(),
            seen,
            scope,
            epoch,
            generation,
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

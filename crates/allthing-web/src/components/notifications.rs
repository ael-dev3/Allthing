// SPDX-License-Identifier: AGPL-3.0-or-later
//! The signed-in reader's notifications, as the app holds them: the times
//! of the first page of `/api/v1/notifications` (`Times`), read when the
//! app starts, every three minutes while the page is in view, and when it
//! is back in view or its window regains focus (a read within a minute is
//! reused: `api`), and the newest time the reader has seen, which this
//! browser caches by account (`allthing-notifications-seen` in localStorage)
//! and SpacetimeDB shares with the account's other verified sessions. The header's bell counts the groups newer than
//! that, and the notifications page (`pages::notifications`), which reads
//! the whole payload from the same answer, marks them seen. Not in the
//! static copy, which has no sign-in.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use allthing_core::notification_state::valid_seen;
use allthing_core::notifications::{NOTIFICATIONS_PATH, Times, badge, bell_label};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::{self, Failure};
use crate::components::signin::use_session;
use crate::util::{BASE, document, notifications_href};

/// How often the bell asks again while the page is in view.
const EVERY: Duration = Duration::from_secs(180);
/// Where this browser keeps, by account, the newest time its reader has
/// seen: `{"<fid>": <Unix seconds>}`.
const SEEN_KEY: &str = "allthing-notifications-seen";

/// The reader's notifications and what they have seen, held by the app:
/// one signal, so the app carries one more kind of signal, not four.
#[derive(Clone, Copy)]
pub struct Notifications(RwSignal<State>);

#[derive(Clone, Default)]
struct State {
    /// Whose they are: the account signed in, if any.
    whose: Option<u64>,
    /// The first page's times, as last read for `whose`.
    times: Option<Arc<Times>>,
    /// Why the last read failed, if it did and nothing was read since.
    failed: Option<Failure>,
    /// One refresh at a time, including calls from the page opening and
    /// from focus while the initial read is still in flight.
    loading: bool,
    /// The newest time `whose` has seen (Unix seconds), 0 for never.
    seen: i64,
    /// Fences in-flight reads even if A signs out and signs back in as A.
    generation: u64,
    /// Prior remote state, captured before this visit's pending read is sent.
    remote_baseline: Option<i64>,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// What this browser keeps of each account's seen time.
fn kept() -> BTreeMap<String, i64> {
    let text = storage().and_then(|s| s.get_item(SEEN_KEY).ok().flatten());
    text.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn seen_of(fid: u64) -> i64 {
    valid_seen(kept().get(&fid.to_string()).copied().unwrap_or(0), now())
}

fn now() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

fn keep_seen(fid: u64, at: i64) -> i64 {
    let at = valid_seen(at, now()).max(seen_of(fid));
    let mut all = kept();
    all.insert(fid.to_string(), at);
    if let (Some(storage), Ok(text)) = (storage(), serde_json::to_string(&all)) {
        let _ = storage.set_item(SEEN_KEY, &text);
    }
    at
}

impl Notifications {
    pub fn new() -> Notifications {
        let session = use_session();
        let whose = session.user.get_untracked().map(|user| user.fid);
        let state = State { whose, seen: whose.map_or(0, seen_of), ..State::default() };
        let notifications = Notifications(RwSignal::new(state));
        // Someone signs in or out, here or in another tab: start over, and
        // forget the last reader's pages.
        Effect::new(move |_| {
            let fid = session.user.with(|user| user.as_ref().map(|user| user.fid));
            if notifications.whose() != fid {
                api::forget(NOTIFICATIONS_PATH);
                api::forget(allthing_core::following::FOLLOWING_PATH);
                let generation = notifications.generation().wrapping_add(1);
                notifications.0.set(State { whose: fid, seen: fid.map_or(0, seen_of), generation, ..State::default() });
            }
            crate::notification_sync::account(fid, notifications);
            if fid.is_some() {
                notifications.refresh();
            }
        });
        set_interval(
            move || {
                if !document().hidden() {
                    notifications.refresh();
                }
            },
            EVERY,
        );
        // Back in view, or the window focused again: what came meanwhile.
        let woken = move |_| {
            if !document().hidden() {
                notifications.refresh();
                crate::notification_sync::wake();
            }
        };
        let _ = window_event_listener_untyped("visibilitychange", woken);
        let _ = window_event_listener_untyped("focus", woken);
        let _ = window_event_listener_untyped("online", move |_| crate::notification_sync::wake());
        // Seen in another of this browser's tabs.
        let _ = window_event_listener(leptos::ev::storage, move |ev| {
            if ev.key().as_deref() == Some(SEEN_KEY)
                && let Some(fid) = notifications.whose()
            {
                let seen = seen_of(fid).max(notifications.seen_by(fid));
                if seen > notifications.seen_by(fid) {
                    notifications.0.update(|state| state.seen = seen);
                }
                crate::notification_sync::seen(fid, seen);
            }
        });
        notifications
    }

    fn whose(self) -> Option<u64> {
        self.0.with_untracked(|state| state.whose)
    }

    /// Whether these are `fid`'s: the account the app knows is signed in
    /// (untracked).
    pub fn are_for(self, fid: u64) -> bool {
        self.whose() == Some(fid)
    }

    pub fn generation(self) -> u64 {
        self.0.with_untracked(|state| state.generation)
    }

    pub fn same_session(self, fid: u64, generation: u64) -> bool {
        self.are_for(fid) && self.generation() == generation
    }

    pub fn sync_baseline(self, fid: u64) -> Option<i64> {
        self.0.with(|state| if state.whose == Some(fid) { state.remote_baseline } else { None })
    }

    /// A private subscription has advanced. Do not echo it as another write.
    pub fn synced(self, fid: u64, seen: i64, baseline: Option<i64>) {
        if !self.are_for(fid) {
            return;
        }
        let seen = keep_seen(fid, seen).max(self.seen_by(fid));
        if self.0.with_untracked(|s| seen > s.seen || (s.remote_baseline.is_none() && baseline.is_some())) {
            self.0.update(|state| {
                state.seen = seen;
                if state.remote_baseline.is_none() {
                    state.remote_baseline = baseline;
                }
            });
        }
    }

    /// The first page's times once its latest refresh succeeded (tracked).
    /// Old times must not start a page read behind a refresh that can fail.
    pub fn times(self) -> Option<Arc<Times>> {
        self.0.with(|state| if state.loading || state.failed.is_some() { None } else { state.times.clone() })
    }

    /// Why the last read failed, if it did (tracked).
    pub fn failed(self) -> Option<Failure> {
        self.0.with(|state| state.failed.clone())
    }

    /// The newest time `fid` has seen, Unix seconds (untracked): as the app
    /// holds it for the reader signed in, or else as this browser keeps it.
    pub fn seen_by(self, fid: u64) -> i64 {
        self.0.with_untracked(|state| (state.whose == Some(fid)).then_some(state.seen)).unwrap_or_else(|| seen_of(fid))
    }

    /// Opening the page shares the bell's initial read, including a failure
    /// that arrived before the page's code. Only Try again or a later refresh
    /// should retry that failure; a fresh answer is already ready to reuse.
    pub fn opened(self) {
        if self.0.with_untracked(|state| state.loading || state.failed.is_some())
            || api::peek::<Times>(NOTIFICATIONS_PATH).is_some()
        {
            return;
        }
        self.refresh();
    }

    /// Read the first page again for the reader signed in, unless a read
    /// within a minute can be reused or is on its way (`api::get`).
    pub fn refresh(self) {
        let Some(fid) = self.0.with_untracked(|state| state.whose.filter(|_| !state.loading)) else { return };
        let generation = self.generation();
        // Coalesce before spawning: a second caller otherwise waits in
        // api::get for the first read. A throttled tab can resume that
        // waiter after the cache has expired and start a redundant read.
        self.0.update(|state| state.loading = true);
        spawn_local(async move {
            let read = api::get::<Times>(NOTIFICATIONS_PATH).await;
            // Signed out, or someone else, meanwhile.
            if !self.same_session(fid, generation) {
                return;
            }
            self.0.update(|state| {
                state.loading = false;
                match read {
                    // The same times again redraw nothing.
                    Ok(fetched) if state.failed.is_none() && state.times.as_deref() == Some(&*fetched.data) => {}
                    Ok(fetched) => {
                        state.times = Some(fetched.data);
                        state.failed = None;
                    }
                    Err(failure) => state.failed = Some(failure),
                }
            });
        });
    }

    /// Read again after a failure, showing it's under way.
    pub fn retry(self) {
        self.0.update(|state| state.failed = None);
        self.refresh();
    }

    /// `fid` has seen everything up to `newest` (Unix seconds): the bell
    /// clears, here and in the browser's other tabs.
    pub fn mark_seen(self, fid: u64, newest: i64) {
        let newest = valid_seen(newest, now());
        if !self.are_for(fid) || newest <= self.seen_by(fid) {
            return;
        }
        let newest = keep_seen(fid, newest);
        self.0.update(|state| state.seen = newest);
        crate::notification_sync::seen(fid, newest);
    }

    /// How many groups of the first page the reader hasn't seen (tracked).
    pub fn unread(self) -> usize {
        self.0.with(|state| {
            state.times.as_ref().map_or(0, |times| {
                times
                    .groups
                    .iter()
                    .filter(|group| group.ts > state.seen && !crate::moderation::hides_group_time(group))
                    .count()
            })
        })
    }
}

pub fn use_notifications() -> Option<Notifications> {
    use_context::<Notifications>()
}

/// A bell, like the Farcaster app's.
const BELL: &str = "M4.4 10.6V7.1a3.6 3.6 0 0 1 7.2 0v3.5l1.3 1.6H3.1zM6.5 13.4a1.6 1.6 0 0 0 3 0";

/// The header's bell, for a signed-in reader: a link to the notifications,
/// with how many the reader hasn't seen ("9+" past nine), which screen
/// readers hear exactly.
#[component]
pub fn Bell() -> impl IntoView {
    let session = use_session();
    let Some(notifications) = use_notifications() else { return ().into_any() };
    let location = use_location();
    let here = move || {
        let path = location.pathname.get();
        path.strip_prefix(BASE).unwrap_or(&path) == "/notifications"
    };
    (move || {
        session.user.with(Option::is_some).then(|| {
            view! {
                <a
                    class="bell"
                    href=notifications_href()
                    aria-label=move || bell_label(notifications.unread())
                    aria-current=move || here().then_some("page")
                >
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=BELL/></svg>
                    {move || badge(notifications.unread()).map(|n| view! { <span class="bell-badge" aria-hidden="true">{n}</span> })}
                </a>
            }
        })
    })
    .into_any()
}

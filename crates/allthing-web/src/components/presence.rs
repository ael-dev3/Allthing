// SPDX-License-Identifier: AGPL-3.0-or-later
//! Who is reading Allthing now: the header's small "● 12", and its
//! popover of those shown by name (`allthing_core::presence`). In front of
//! the Worker only; the static copy has none.
//!
//! The browser tells the Worker it is here with a beat (`POST
//! /api/v1/presence`), never as part of a page view: the first once the
//! page's own call is in and the browser is idle, two seconds after the app
//! starts at the soonest, then one every `next` seconds, the interval the
//! Worker answers with, while the tab is visible, online and in use (input
//! in the last five minutes, or a video playing). One tab beats for the
//! whole browser: localStorage `allthing.presence` keeps the day's random
//! id, when a tab last beat, the interval and the answer, and a tab whose
//! turn hasn't come shows that answer, kept current by the `storage` event.
//! The chip shows once an answer is in or kept, and stays away for a while
//! where the Worker says presence is off or isn't set up.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use allthing_core::format::full_number;
use allthing_core::model::UserSummary;
use allthing_core::presence::{
    Answer, Beat, IDLE_MS, MAX_NEXT, MIN_NEXT, PRESENCE_PATH, chip_name, count_label, should_beat, todays_id, utc_day,
    with_me,
};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use crate::api::{self, Failure};
use crate::components::chips::Avatar;
use crate::components::popover::Popover;
use crate::components::signin::use_session;
use crate::util::{can_hover, document, user_href};

/// What a browser keeps for its tabs and its next visits.
const KEY: &str = "allthing.presence";
/// `"1"` while a signed-in reader asks not to be shown by name.
const HIDDEN_KEY: &str = "allthing.presence-hidden";
/// The first beat waits this long after the app starts, at the least…
const FIRST_AFTER: Duration = Duration::from_millis(2000);
/// …and while the page's own reads are on their way, up to this long.
const QUIET_WAIT_MS: f64 = 15_000.0;
/// Where the Worker says presence is off, or isn't set up, no beat goes and
/// the chip stays away for this long.
const OFF_MS: f64 = 6.0 * 60.0 * 60.0 * 1000.0;
/// After an answer it can't use, this long.
const REFUSED_MS: f64 = 60.0 * 60.0 * 1000.0;
/// A beat that didn't reach the Worker, or found it busy, goes again after
/// this long at the soonest.
const RETRY_MS: f64 = 60_000.0;
/// Input this soon after the last is not news.
const INPUT_EVERY_MS: f64 = 1000.0;

/// What `allthing.presence` holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Kept {
    /// The browser's random id for the day, and that day (`utc_day`).
    #[serde(default)]
    id: String,
    #[serde(default)]
    day: i64,
    /// When a tab of this browser last beat, in Unix milliseconds. (Whole
    /// ones: printing a float would bring in a float printer.)
    #[serde(default)]
    at: u64,
    /// The interval the Worker last answered with, in seconds.
    #[serde(default)]
    next: u32,
    #[serde(default)]
    answer: Option<Answer>,
    /// Until when presence is off here, in Unix milliseconds.
    #[serde(default)]
    off: u64,
}

thread_local! {
    /// What this tab last kept, for a browser whose localStorage can't be
    /// read: its beats are spaced all the same.
    static KEPT: RefCell<Option<Kept>> = const { RefCell::new(None) };
    /// The next look at whether to beat.
    static TIMER: RefCell<Option<TimeoutHandle>> = const { RefCell::new(None) };
    /// When the reader last did something here, in Unix milliseconds.
    static LAST_INPUT: Cell<f64> = const { Cell::new(0.0) };
    /// This tab's share of the interval, 0.9 to 1.1, so two tabs don't
    /// both look at once.
    static JITTER: Cell<f64> = const { Cell::new(1.0) };
    /// A beat of this tab's is on its way.
    static BEATING: Cell<bool> = const { Cell::new(false) };
    /// The first beat's wait is over.
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

/// What the chip shows, for the header.
#[derive(Clone, Copy)]
pub struct Presence {
    /// The latest answer this browser has, while presence is on here.
    answer: RwSignal<Option<Answer>>,
    /// The signed-in reader asked not to be shown by name.
    hidden: RwSignal<bool>,
}

impl Presence {
    /// The chip, from the answer kept if there is one, and the beats, the
    /// first after the page's own call.
    pub fn new() -> Presence {
        let now = js_sys::Date::now();
        let Kept { answer, off, .. } = read();
        let presence =
            Presence { answer: RwSignal::new(answer.filter(|_| off as f64 <= now)), hidden: RwSignal::new(hidden()) };
        LAST_INPUT.set(now);
        JITTER.set(jitter());
        presence.listen();
        set_timeout(move || presence.when_quiet(now + QUIET_WAIT_MS), FIRST_AFTER);
        // Signing in or out: the reader is shown, or no longer, at once.
        let session = use_session();
        Effect::new(move |seen: Option<Option<u64>>| {
            let fid = session.user.with(|user| user.as_ref().map(|user| user.fid));
            if seen.is_some_and(|seen| seen != fid) {
                presence.tick(true);
            }
            fid
        });
        presence
    }

    /// Once the page's own reads are in (or after a while), when the browser
    /// is idle: the first look.
    fn when_quiet(self, until: f64) {
        if api::quiet() || js_sys::Date::now() >= until {
            crate::components::cast::idle(move || {
                STARTED.set(true);
                self.tick(false);
            });
        } else {
            set_timeout(move || self.when_quiet(until), Duration::from_millis(250));
        }
    }

    /// Input, a tab shown or hidden, the network back or gone, and another
    /// tab's beat: each may start or stop the beats.
    fn listen(self) {
        let input = move |_: web_sys::Event| {
            let now = js_sys::Date::now();
            if now - LAST_INPUT.get() < INPUT_EVERY_MS {
                return;
            }
            LAST_INPUT.set(now);
            // Back from idle: beat at once if one is due.
            if TIMER.with(|timer| timer.borrow().is_none()) {
                self.tick(false);
            }
        };
        for event in ["pointerdown", "pointermove", "keydown", "wheel", "touchstart", "scroll"] {
            on_window(event, input);
        }
        for event in ["visibilitychange", "online", "offline"] {
            on_window(event, move |_| self.tick(false));
        }
        on_window("storage", move |ev| {
            let key =
                js_sys::Reflect::get(&ev, &wasm_bindgen::JsValue::from_str("key")).ok().and_then(|key| key.as_string());
            match key.as_deref() {
                // Another tab's beat, or the storage cleared.
                Some(KEY) | None => self.tick(false),
                Some(HIDDEN_KEY) => self.hidden.set(hidden()),
                Some(_) => {}
            }
        });
    }

    /// Beat if it is this browser's turn (or at once, `now_please`, for a
    /// change the reader made), else show what another tab heard, and look
    /// again when the turn comes. Nothing while the tab is hidden, offline
    /// or idle: the listeners look again.
    fn tick(self, now_please: bool) {
        clear_timer();
        let now = js_sys::Date::now();
        let kept = read();
        if kept.off as f64 > now {
            if self.answer.with_untracked(Option::is_some) {
                self.answer.set(None);
            }
            return;
        }
        if !STARTED.get() || BEATING.get() {
            self.adopt(&kept);
            return;
        }
        if video_playing() {
            LAST_INPUT.set(now);
        }
        let visible = !document().hidden();
        let online = window().navigator().on_line();
        let next_ms = f64::from(kept.next.clamp(MIN_NEXT, MAX_NEXT)) * 1000.0 * JITTER.get();
        let at = kept.at as f64;
        let last = (kept.at > 0).then_some(at);
        let due = should_beat(now, last, LAST_INPUT.get(), visible, online, next_ms);
        if (due || now_please) && visible && online {
            self.beat(kept, now);
            return;
        }
        self.adopt(&kept);
        if visible && online && now - LAST_INPUT.get() <= IDLE_MS {
            let wait = (at + next_ms - now).max(1000.0);
            let handle = set_timeout_with_handle(
                move || {
                    TIMER.with(|timer| timer.borrow_mut().take());
                    self.tick(false);
                },
                Duration::from_millis(wait as u64),
            );
            TIMER.with(|timer| *timer.borrow_mut() = handle.ok());
        }
    }

    /// Show the answer another tab heard, if it is news.
    fn adopt(self, kept: &Kept) {
        if kept.answer.is_some() && self.answer.with_untracked(|answer| *answer != kept.answer) {
            self.answer.set(kept.answer.clone());
        }
    }

    /// Tell the Worker this browser is here, and keep what it answers. The
    /// beat is claimed in localStorage first, so another tab looking
    /// meanwhile waits for its answer instead of beating too.
    fn beat(self, mut kept: Kept, now: f64) {
        let today = utc_day(now);
        let Some(id) = todays_id(Some((kept.id.as_str(), kept.day)), today).or_else(random_id) else { return };
        kept.id.clone_from(&id);
        kept.day = today;
        kept.at = now as u64;
        write(&kept);
        BEATING.set(true);
        let beat = Beat { id, hidden: hidden() };
        wasm_bindgen_futures::spawn_local(async move {
            let answer = api::post::<_, Answer>(PRESENCE_PATH, &beat).await;
            BEATING.set(false);
            let now = js_sys::Date::now();
            let mut kept = read();
            match answer {
                Ok(answer) => {
                    kept.next = answer.next;
                    kept.answer = Some(answer);
                    kept.off = 0;
                    JITTER.set(jitter());
                }
                // Off here, or not set up: no chip, and no beats, for a while.
                Err(Failure::NotFound(_) | Failure::Busy { retry_after: None, .. }) => {
                    kept.answer = None;
                    kept.off = (now + OFF_MS) as u64;
                }
                // Not reached, or busy: again in a minute, or when it says,
                // within the interval. (The turn is dated back so that it
                // comes then.)
                Err(failure) if failure.is_transient() => {
                    let next_ms = f64::from(kept.next.clamp(MIN_NEXT, MAX_NEXT)) * 1000.0;
                    let wait = match failure {
                        Failure::Busy { retry_after: Some(seconds), .. } => (f64::from(seconds) * 1000.0).max(RETRY_MS),
                        _ => RETRY_MS,
                    };
                    kept.at = (now + wait.min(next_ms) - next_ms) as u64;
                }
                // Refused: a fresh id, an hour on.
                Err(_) => {
                    kept.id.clear();
                    kept.answer = None;
                    kept.off = (now + REFUSED_MS) as u64;
                }
            }
            write(&kept);
            self.tick(false);
        });
    }

    /// Show the signed-in reader by name here, or not; and beat at once.
    fn set_hidden(self, hide: bool) {
        if let Some(storage) = storage() {
            let _ = if hide { storage.set_item(HIDDEN_KEY, "1") } else { storage.remove_item(HIDDEN_KEY) };
        }
        self.hidden.set(hide);
        self.tick(true);
    }
}

/// The header's chip, after Home: how many are reading Allthing now; on
/// hover, a click, a tap or Enter, those of them shown by name.
#[component]
pub fn PresenceChip() -> impl IntoView {
    let presence = expect_context::<Presence>();
    let session = use_session();
    // Those shown by name and the count: the signed-in reader first if the
    // answer doesn't have them yet, and nowhere if they hide.
    let seen = Memo::new(move |_| {
        let answer = presence.answer.get()?;
        let me = session.user.get();
        let hidden = presence.hidden.get();
        let (mut readers, online) = with_me(&answer, me.as_ref().filter(|_| !hidden));
        if let Some(me) = me.as_ref().filter(|_| hidden) {
            readers.retain(|reader| reader.fid != me.fid);
        }
        Some((readers, online))
    });
    let on = Memo::new(move |_| seen.with(Option::is_some));
    let online = Memo::new(move |_| seen.with(|seen| seen.as_ref().map_or(1, |(_, online)| *online)));
    let count = Memo::new(move |_| count_label(online.get()));
    move || {
        on.get().then(|| {
            view! {
                <Popover
                    label="Who's reading Allthing now"
                    trigger_label=Signal::derive(move || chip_name(online.get()))
                    trigger_class="presence-chip"
                    class="presence"
                    panel_class="presence-panel"
                    hover=true
                    no_focus_open=true
                    trigger=move || {
                        view! {
                            <span class="presence-dot" aria-hidden="true"></span>
                            // A new count fades in.
                            {move || {
                                let count = count.get();
                                view! { <span class="presence-count">{count}</span> }
                            }}
                        }
                    }
                    content=move || panel(presence, seen, online)
                />
            }
        })
    }
}

/// The popover: how many are here, those shown by name, how many more, and
/// for a signed-in reader, whether to show them.
fn panel(presence: Presence, seen: Memo<Option<(Vec<UserSummary>, u64)>>, online: Memo<u64>) -> AnyView {
    let session = use_session();
    // The name of the picture the pointer or focus is on.
    let caption = RwSignal::new(None::<String>);
    let point = move |ev: web_sys::Event| caption.set(name_at(&ev));
    let named = move || {
        let (readers, online) = seen.get().unwrap_or_default();
        if readers.is_empty() {
            return None;
        }
        let more = online.saturating_sub(readers.len() as u64);
        let pictures = readers
            .into_iter()
            .map(|reader| {
                view! {
                    <li>
                        <a href=user_href(&reader.username) aria-label=format!("@{}", reader.username)>
                            <Avatar url=reader.pfp_url size=24/>
                        </a>
                    </li>
                }
            })
            .collect_view();
        Some(view! {
            <ul
                class="presence-readers"
                on:mouseover=move |ev| point(ev.into())
                on:focusin=move |ev| point(ev.into())
                on:mouseleave=move |_| caption.set(None)
                on:focusout=move |_| caption.set(None)
            >
                {pictures}
            </ul>
            {(more > 0 || can_hover()).then(|| {
                view! {
                    <p class="presence-foot">
                        <span>{(more > 0).then(|| format!("and {} more", full_number(more)))}</span>
                        <span class="presence-caption" aria-hidden="true">{move || caption.get()}</span>
                    </p>
                }
            })}
        })
    };
    view! {
        <p class="presence-head">{move || format!("{} reading Allthing now", full_number(online.get()))}</p>
        {named}
        {move || {
            session.user.with(Option::is_some).then(|| {
                view! {
                    <button
                        type="button"
                        class="presence-me"
                        role="switch"
                        aria-checked=move || (!presence.hidden.get()).to_string()
                        on:click=move |_| presence.set_hidden(!presence.hidden.get_untracked())
                    >
                        "Show me here"
                        <span class="presence-switch" aria-hidden="true"></span>
                    </button>
                }
            })
        }}
    }
    .into_any()
}

/// The name of the picture an event is on, if it is on one.
fn name_at(ev: &web_sys::Event) -> Option<String> {
    let target = ev.target()?.dyn_into::<web_sys::Element>().ok()?;
    target.closest("a[aria-label]").ok()??.get_attribute("aria-label")
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn read() -> Kept {
    storage()
        .and_then(|storage| storage.get_item(KEY).ok().flatten())
        .and_then(|json| serde_json::from_str(&json).ok())
        .or_else(|| KEPT.with(|kept| kept.borrow().clone()))
        .unwrap_or_default()
}

fn write(kept: &Kept) {
    KEPT.with(|mine| *mine.borrow_mut() = Some(kept.clone()));
    if let (Some(storage), Ok(json)) = (storage(), serde_json::to_string(kept)) {
        let _ = storage.set_item(KEY, &json);
    }
}

/// Whether the signed-in reader asked not to be shown by name.
fn hidden() -> bool {
    storage().and_then(|storage| storage.get_item(HIDDEN_KEY).ok().flatten()).is_some_and(|value| value == "1")
}

/// A fresh random id: 16 random bytes as 32 lowercase hex characters.
fn random_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    window().crypto().ok()?.get_random_values_with_u8_array(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn jitter() -> f64 {
    0.9 + 0.2 * js_sys::Math::random()
}

fn clear_timer() {
    if let Some(handle) = TIMER.with(|timer| timer.borrow_mut().take()) {
        handle.clear();
    }
}

/// Whether a video on the page is playing: watching is using the page.
fn video_playing() -> bool {
    let Ok(videos) = document().query_selector_all("video") else { return false };
    (0..videos.length())
        .filter_map(|i| videos.item(i)?.dyn_into::<web_sys::HtmlMediaElement>().ok())
        .any(|video| !video.paused() && !video.ended())
}

/// Listen to `event` on the window for the app's lifetime: passive, and in
/// the capture phase, so scrolls inside the page are heard too.
fn on_window(event: &str, listen: impl Fn(web_sys::Event) + 'static) {
    let listener = Closure::<dyn Fn(web_sys::Event)>::new(listen);
    let options = web_sys::AddEventListenerOptions::new();
    options.set_passive(true);
    options.set_capture(true);
    let _ = window().add_event_listener_with_callback_and_add_event_listener_options(
        event,
        listener.as_ref().unchecked_ref(),
        &options,
    );
    listener.forget();
}

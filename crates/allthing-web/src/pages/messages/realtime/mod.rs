// SPDX-License-Identifier: AGPL-3.0-or-later
//! Messages on SpacetimeDB: the inbox of /messages, a part of its own
//! (`crate::split`), fetched with the page, as is the SpacetimeDB bridge
//! script after it (`live`) once a signed-in reader is on it.

mod chat;
mod encrypted;
mod inbox;
mod live;

use std::time::Duration;

use allthing_core::time::iso_utc;
use leptos::prelude::*;
use leptos::wasm_split::{SplitLoaderError, wasm_split};

use super::people::People;
use crate::components::chips::Avatar;
use crate::components::signin::use_session;
use crate::util::user_href;
use live::{Live, Phase, live};

/// The inbox's styles, added to the page once, by this part: no other page
/// downloads them.
const STYLE: &str = include_str!("../../../../../../style/realtime.css");

fn add_style() {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    if document.get_element_by_id("rt-style").is_some() {
        return;
    }
    if let (Ok(style), Ok(Some(head))) = (document.create_element("style"), document.query_selector("head")) {
        style.set_id("rt-style");
        style.set_text_content(Some(STYLE));
        let _ = head.append_child(&style);
    }
}

/// The inbox: code fetched the first time it's needed.
#[wasm_split(realtime, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_realtime))]
pub fn inbox() -> Result<AnyView, SplitLoaderError> {
    Ok(view! { <Realtime/> }.into_any())
}

#[component]
fn Realtime() -> impl IntoView {
    add_style();
    let session = use_session();
    let me = session.user.get_untracked();
    let people = People::new(me);
    move || match session.user.with(|u| u.as_ref().map(|u| u.fid)) {
        None => view! { <SignedOut/> }.into_any(),
        Some(fid) => view! { <inbox::Inbox live=live(fid) people/> }.into_any(),
    }
}

#[component]
fn SignedOut() -> impl IntoView {
    let session = use_session();
    view! {
        <section class="dm-panel" aria-labelledby="rt-signed-out">
            <h2 id="rt-signed-out">"Sign in to see your messages"</h2>
            <p>"Chat in Allthing's room and send end-to-end encrypted messages. They live on Allthing's own database, not on Farcaster: nothing you send here is posted."</p>
            <div class="dm-actions">
                <button type="button" class="button primary" on:click=move |_| session.sign_in()>"Sign in"</button>
            </div>
        </section>
    }
}

/// Where the connection is, while it isn't ready, and whether the account
/// is verified once it is.
#[component]
fn Status(live: Live) -> impl IntoView {
    let phase = live.phase.clone();
    move || {
        match phase.get() {
        Phase::Connecting => view! { <p class="rt-status muted" role="status">"Connecting…"</p> }.into_any(),
        Phase::Verifying => view! { <p class="rt-status muted" role="status">"Verifying your Farcaster account…"</p> }.into_any(),
        Phase::Ready { verified: true } => ().into_any(),
        Phase::Ready { verified: false } => view! {
            <p class="rt-status rt-unverified" role="status">
                "Accounts can't be verified here yet, so names on this page are as people claim them."
            </p>
        }
        .into_any(),
        Phase::Failed(why) => view! {
            <p class="rt-status dm-problem" role="alert">{why}" "
                <button type="button" class="button" on:click=|_| { let _ = web_sys::window().map(|w| w.location().reload()); }>"Try again"</button>
            </p>
        }
        .into_any(),
    }
    }
}

/// Who sent something: their picture and name, linked to their profile,
/// and "unverified" when the database hasn't verified their account.
#[component]
fn Who(people: People, fid: u64, verified: bool) -> impl IntoView {
    let user = move || people.get(fid);
    view! {
        <a class="rt-who" href=move || user().map_or_else(|| user_href(&format!("fid:{fid}")), |u| user_href(&u.username))>
            {move || view! { <Avatar url=user().and_then(|u| u.pfp_url) size=24/> }}
            <span class="rt-name">{move || user().map_or_else(|| format!("fid {fid}"), |u| u.name().to_owned())}</span>
        </a>
        {(!verified).then(|| view! { <span class="rt-badge">"unverified"</span> })}
    }
}

/// A clock for what expires (typing lines), ticking every 2 s while shown.
#[derive(Clone, Copy)]
struct Clock(RwSignal<f64>);

impl Clock {
    fn new() -> Clock {
        let now = RwSignal::new(js_sys::Date::now());
        if let Ok(handle) = set_interval_with_handle(move || now.set(js_sys::Date::now()), Duration::from_secs(2)) {
            on_cleanup(move || handle.clear());
        }
        Clock(now)
    }

    /// Now, in microseconds since the Unix epoch, as the database counts.
    fn now_us(self) -> i64 {
        (self.0.get() * 1000.0) as i64
    }
}

/// A time in microseconds: its ISO form, and as the page shows it.
fn time_label(at_us: i64) -> (String, String) {
    let at = at_us / 1_000_000;
    (iso_utc(at), short_time(at))
}

/// A time as a list or a message shows it, in the reader's time zone:
/// "09:41" today, "21 Oct" this year, "21 Oct 2025" before.
fn short_time(at: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at as f64 * 1000.0));
    let today = js_sys::Date::new_0();
    let options = js_sys::Object::new();
    let fields: &[(&str, &str)] = if date.to_date_string() == today.to_date_string() {
        &[("hour", "2-digit"), ("minute", "2-digit"), ("hourCycle", "h23")]
    } else if date.get_full_year() == today.get_full_year() {
        &[("day", "numeric"), ("month", "short")]
    } else {
        &[("day", "numeric"), ("month", "short"), ("year", "numeric")]
    };
    for (key, value) in fields {
        let _ = js_sys::Reflect::set(&options, &(*key).into(), &(*value).into());
    }
    date.to_locale_string("en-GB", &options).into()
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! Small browser helpers and link builders.

use allthing_core::channels::is_channel_id;
use allthing_core::params::percent_encode;
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::{use_location, use_navigate};
use wasm_bindgen::JsCast;

/// Unix seconds, from the browser clock.
pub fn now() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

pub fn document() -> web_sys::Document {
    web_sys::window().and_then(|w| w.document()).expect("document")
}

pub fn set_title(title: &str) {
    document().set_title(&if title.is_empty() { "Allthing".to_owned() } else { format!("{title} · Allthing") });
}

pub fn prefers_reduced_motion() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-reduced-motion: reduce)").ok().flatten())
        .is_some_and(|m| m.matches())
}

/// Whether the reader asked to save data (Save-Data: a phone's data saver,
/// or Chrome's Lite mode): what would move while in view stays still.
#[cfg(not(feature = "static-host"))]
pub fn saves_data() -> bool {
    web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w.navigator(), &"connection".into()).ok())
        .filter(|connection| connection.is_object())
        .and_then(|connection| js_sys::Reflect::get(&connection, &"saveData".into()).ok())
        .and_then(|saves| saves.as_bool())
        .unwrap_or(false)
}

/// Whether the primary pointer can hover (desktop), so hover previews make sense.
pub fn can_hover() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(hover: hover) and (pointer: fine)").ok().flatten())
        .is_some_and(|m| m.matches())
}

/// Where the site is served from: "" on allthing.social, "/Allthing" for the
/// GitHub Pages build (set by `ALLTHING_BASE_PATH` at compile time).
pub const BASE: &str = match option_env!("ALLTHING_BASE_PATH") {
    Some(base) => base,
    None => "",
};

/// A site path, such as `/channels`, under [`BASE`].
pub fn href(path: &str) -> String {
    format!("{BASE}{path}")
}

/// Goes to a path built with [`href`], as a click on a link to it does. The
/// router's own `navigate` puts its base before a path it resolves, which
/// sent the static copy from `/Allthing/...` to `/Allthing/Allthing/...`.
pub fn use_go() -> impl Fn(&str, NavigateOptions) + Clone + 'static {
    let navigate = use_navigate();
    move |to: &str, options: NavigateOptions| {
        // Any page the app opens takes over from one still waiting to (nav.rs).
        crate::nav::cancel();
        navigate(to, NavigateOptions { resolve: false, ..options })
    }
}

/// A query value decoded exactly once. The router's ParamsMap also
/// unescapes values already decoded by URLSearchParams, which changes
/// literal escapes such as "%2F" inside searches and parent URLs.
pub fn use_query_param(name: &'static str) -> Memo<Option<String>> {
    let location = use_location();
    Memo::new(move |_| web_sys::UrlSearchParams::new_with_str(&location.search.get()).ok().and_then(|q| q.get(name)))
}

/// Keep Tab and Shift+Tab inside `root`, a modal dialog: from its last
/// control (`controls`, a selector) round to its first, and back. (The
/// static copy has no dialogs that need it.)
#[cfg(not(feature = "static-host"))]
pub fn trap_tab(root: &web_sys::Element, ev: &web_sys::KeyboardEvent, controls: &str) {
    let Ok(nodes) = root.query_selector_all(controls) else { return };
    let first = nodes.get(0).and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok());
    let last = nodes.get(nodes.length().saturating_sub(1)).and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok());
    let active = document().active_element();
    let at = |el: &Option<web_sys::HtmlElement>| {
        el.as_ref().map(|e| e.unchecked_ref::<web_sys::Element>().clone()) == active
    };
    let (from, to) = if ev.shift_key() { (&first, &last) } else { (&last, &first) };
    if at(from) {
        ev.prevent_default();
        if let Some(to) = to {
            let _ = to.focus();
        }
    }
}

pub fn element_by_id<T: JsCast>(id: &str) -> Option<T> {
    document().get_element_by_id(id)?.dyn_into::<T>().ok()
}

pub fn channel_href(key: &str) -> String {
    if is_channel_id(key) { href(&format!("/c/{key}")) } else { href(&format!("/c?id={}", percent_encode(key))) }
}

pub fn cast_href(hash: &str) -> String {
    href(&format!("/p/{hash}"))
}

/// Farcaster's daily active accounts.
pub fn dau_href() -> String {
    href("/dau")
}

/// The reader's scheduled casts.
#[cfg(not(feature = "static-host"))]
pub fn scheduled_href() -> String {
    href("/scheduled")
}

/// The reader's Direct Casts: their conversations, and one open
/// (`?c=<conversation id>`, or `?g=<group id>`), or a new message being
/// written (`?new=1`).
#[cfg(not(feature = "static-host"))]
pub fn messages_href() -> String {
    href("/messages")
}

/// A tab of the Messages page: `chat` (the general room) or `encrypted`;
/// none for Direct Casts.
#[cfg(not(feature = "static-host"))]
pub fn messages_tab_href(tab: Option<&str>) -> String {
    with_query(&messages_href(), &[("tab", tab.map(str::to_owned))])
}

/// Encrypted messages, with `fid` open, or none.
#[cfg(not(feature = "static-host"))]
pub fn encrypted_href(fid: Option<u64>) -> String {
    with_query(&messages_href(), &[("tab", Some("encrypted".to_owned())), ("to", fid.map(|f| f.to_string()))])
}

/// One of the reader's conversations, open on the Messages page.
#[cfg(not(feature = "static-host"))]
pub fn conversation_href(conversation_id: &str) -> String {
    with_query(&messages_href(), &[("c", Some(conversation_id.to_owned()))])
}

/// One of the reader's group chats, open on the Messages page.
#[cfg(not(feature = "static-host"))]
pub fn group_href(group_id: &str) -> String {
    with_query(&messages_href(), &[("g", Some(group_id.to_owned()))])
}

/// The Messages page, writing a new message.
#[cfg(not(feature = "static-host"))]
pub fn new_message_href() -> String {
    with_query(&messages_href(), &[("new", Some("1".to_owned()))])
}

/// The reader's notifications.
#[cfg(not(feature = "static-host"))]
pub fn notifications_href() -> String {
    href("/notifications")
}

/// A moment in the reader's own time zone, as scheduled casts show it:
/// "Wed, 21 Oct, 09:00".
#[cfg(not(feature = "static-host"))]
pub fn local_time(unix: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(unix as f64 * 1000.0));
    let options = js_sys::Object::new();
    let fields =
        [("weekday", "short"), ("day", "numeric"), ("month", "short"), ("hour", "2-digit"), ("minute", "2-digit")];
    for (key, value) in fields.into_iter().chain([("hourCycle", "h23")]) {
        let _ = js_sys::Reflect::set(&options, &key.into(), &value.into());
    }
    date.to_locale_string("en-GB", &options).into()
}

/// A moment as a date-and-time field holds it, in the reader's time zone:
/// "2026-10-21T09:00".
#[cfg(not(feature = "static-host"))]
pub fn local_input(unix: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(unix as f64 * 1000.0));
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date(),
        d.get_hours(),
        d.get_minutes()
    )
}

/// The moment a date-and-time field's value names, in the reader's time
/// zone: Unix seconds.
#[cfg(not(feature = "static-host"))]
pub fn from_local_input(value: &str) -> Option<i64> {
    if value.len() < 16 {
        return None;
    }
    let ms = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value)).get_time();
    ms.is_finite().then_some((ms / 1000.0).floor() as i64)
}

/// The reader's time zone, as the browser names it ("Europe/Berlin").
#[cfg(not(feature = "static-host"))]
pub fn time_zone() -> Option<String> {
    let format = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new());
    js_sys::Reflect::get(&format.resolved_options(), &"timeZone".into()).ok()?.as_string()
}

pub fn user_href(username: &str) -> String {
    href(&format!("/u/{}", percent_encode(username)))
}

/// The accounts that follow `username`.
pub fn followers_href(username: &str) -> String {
    format!("{}/followers", user_href(username))
}

/// The accounts `username` follows.
pub fn following_href(username: &str) -> String {
    format!("{}/following", user_href(username))
}

/// Append `?key=value&…`, leaving out empty values.
pub fn with_query(path: &str, pairs: &[(&str, Option<String>)]) -> String {
    let mut out = path.to_owned();
    let mut first = !path.contains('?');
    for (key, value) in pairs {
        if let Some(value) = value.as_deref().filter(|v| !v.is_empty()) {
            out.push(if first { '?' } else { '&' });
            first = false;
            out.push_str(key);
            out.push('=');
            out.push_str(&percent_encode(value));
        }
    }
    out
}

pub const SOURCE_URL: &str = "https://github.com/ael-dev3/Allthing";
/// The GitHub mark, for the links to the source: GitHub's Octicons
/// `mark-github` (MIT), drawn in the text's colour.
pub const GITHUB_MARK: &str = "M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z";

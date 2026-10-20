// SPDX-License-Identifier: AGPL-3.0-or-later
//! Small browser helpers and link builders.

use allthing_core::channels::is_channel_id;
use allthing_core::params::percent_encode;
use leptos::prelude::*;
use leptos_router::hooks::use_location;
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

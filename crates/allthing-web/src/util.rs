// SPDX-License-Identifier: AGPL-3.0-or-later
//! Small browser helpers and link builders.

use allthing_core::channels::is_channel_id;
use allthing_core::params::percent_encode;
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

pub fn element_by_id<T: JsCast>(id: &str) -> Option<T> {
    document().get_element_by_id(id)?.dyn_into::<T>().ok()
}

pub fn channel_href(key: &str) -> String {
    if is_channel_id(key) { href(&format!("/c/{key}")) } else { href(&format!("/c?id={}", percent_encode(key))) }
}

pub fn cast_href(hash: &str) -> String {
    href(&format!("/p/{hash}"))
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

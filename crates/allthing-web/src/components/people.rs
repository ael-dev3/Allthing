// SPDX-License-Identifier: AGPL-3.0-or-later
//! What an account's followers or following page shows until its people
//! have come: its header, its toolbar and rows of people, each as tall as
//! it will be, so that nothing moves as they come. Its code is a part of
//! its own (`crate::pages::follows`); this waits in its place meanwhile.

use std::cell::Cell;

use leptos::prelude::*;

/// A row's least height (`.person-row`, 64 px; 72 on a phone).
const ROW_PX: f64 = 64.0;
/// How long a page takes to come in (`.page > *` in style/allthing.css).
const ENTRANCE_MS: f64 = 420.0;
/// How long after the placeholder came the page taking its place is taken
/// to be taking it, rather than opened anew.
const TAKING_MS: f64 = 10_000.0;

thread_local! {
    /// When the placeholder last came into the page.
    static CAME: Cell<Option<f64>> = const { Cell::new(None) };
}

/// The page before its people: its header, toolbar and rows waiting.
pub fn follows_placeholder() -> impl IntoView {
    CAME.with(|came| came.set(Some(js_sys::Date::now())));
    view! {
        <section class="follows-page" aria-busy="true">
            <div class="follows-head">
                <span class="follows-owner">
                    <span class="sk sk-avatar"></span>
                    <span class="follows-owner-names"><span class="sk sk-title"></span><span class="sk sk-meta"></span></span>
                </span>
                <span class="tabs follows-tabs" aria-hidden="true"><span class="sk sk-tab"></span><span class="sk sk-tab"></span></span>
            </div>
            <div class="follows-toolbar" aria-hidden="true"><span class="follows-search"></span></div>
            {waiting_rows(None)}
        </section>
    }
}

/// The page's entrance as it takes the placeholder's place: on from where
/// the placeholder's had got to, rather than from the start again, so the
/// page doesn't blink as its code comes. `None`, the whole entrance, when
/// no placeholder came before it.
pub fn entrance() -> Option<String> {
    let since = js_sys::Date::now() - CAME.with(Cell::take)?;
    if since < ENTRANCE_MS {
        Some(format!("animation-delay: -{since:.0}ms"))
    } else if since < TAKING_MS {
        Some("animation: none".to_owned())
    } else {
        None
    }
}

/// Rows of people on their way: enough to fill the screen, or as many as
/// the list holds (`count`), if fewer, so what comes after the list is
/// where it will be.
pub fn waiting_rows(count: Option<u64>) -> impl IntoView {
    let screen = web_sys::window().and_then(|w| w.inner_height().ok()).and_then(|h| h.as_f64()).unwrap_or(900.0);
    let fill = (screen / ROW_PX).ceil() as u64;
    let rows = count.map_or(fill, |count| count.min(fill));
    view! {
        <ol class="people-list waiting" aria-hidden="true">
            {(0..rows)
                .map(|_| {
                    view! {
                        <li class="person-row">
                            <span class="sk sk-avatar"></span>
                            <span class="person-row-id"><span class="sk sk-title"></span></span>
                            <span class="person-row-bio"><span class="sk sk-meta"></span></span>
                        </li>
                    }
                })
                .collect_view()}
        </ol>
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! Navigation that feels instant: scroll positions restored on back and
//! forward, view transitions between pages, and cards that open anywhere
//! they are clicked (except on links, controls, media, or selected text).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use leptos::ev;
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::{JsCast, JsValue};

thread_local! {
    /// Last scroll position of each visited page (pathname + query).
    static POSITIONS: RefCell<HashMap<String, f64>> = RefCell::new(HashMap::new());
    /// Set while a restore is in progress, so clamped scrolls aren't saved.
    static RESTORING: Cell<bool> = const { Cell::new(false) };
}

fn location_key() -> String {
    let location = window().location();
    format!("{}{}", location.pathname().unwrap_or_default(), location.search().unwrap_or_default())
}

fn scroll_y() -> f64 {
    window().scroll_y().unwrap_or(0.0)
}

/// Keep scroll positions per page and restore them on back and forward,
/// once the page is tall enough to scroll there (data loads asynchronously).
#[component]
pub fn ScrollMemory() -> impl IntoView {
    let history = window().history().ok();
    if let Some(history) = &history {
        let _ = js_sys::Reflect::set(history, &JsValue::from_str("scrollRestoration"), &JsValue::from_str("manual"));
    }
    let scroll = window_event_listener(ev::scroll, |_| {
        if !RESTORING.with(Cell::get) {
            POSITIONS.with(|p| p.borrow_mut().insert(location_key(), scroll_y()));
        }
    });
    let popstate = window_event_listener(ev::popstate, |_| {
        let Some(target) = POSITIONS.with(|p| p.borrow().get(&location_key()).copied()) else { return };
        RESTORING.with(|r| r.set(true));
        restore(target, 0);
    });
    on_cleanup(move || {
        scroll.remove();
        popstate.remove();
    });
}

/// Try to scroll to `target` for up to ~1.5 s while the page fills in.
fn restore(target: f64, attempt: u32) {
    let height = document().document_element().map(|e| f64::from(e.scroll_height())).unwrap_or(0.0);
    let viewport = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    let reachable = height - viewport >= target - 1.0;
    if reachable || attempt >= 90 {
        window().scroll_to_with_x_and_y(0.0, target);
        RESTORING.with(|r| r.set(false));
        POSITIONS.with(|p| p.borrow_mut().insert(location_key(), scroll_y()));
        return;
    }
    request_animation_frame(move || restore(target, attempt + 1));
}

/// Elements whose clicks belong to themselves, not to the card.
const INTERACTIVE: &str =
    "a, button, input, textarea, select, summary, label, video, iframe, img, .popover, .media-grid, [role=dialog]";

/// A click handler for a whole card: open `href` unless the click was on an
/// interactive element or ended a text selection. Ctrl/⌘-click opens a tab.
pub fn card_click(href: String) -> impl Fn(ev::MouseEvent) + Clone + 'static {
    let navigate = use_navigate();
    move |ev: ev::MouseEvent| {
        if ev.default_prevented() || ev.button() != 0 {
            return;
        }
        let on_control = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| el.closest(INTERACTIVE).ok().flatten())
            .is_some();
        let selecting =
            window().get_selection().ok().flatten().is_some_and(|s| !s.is_collapsed() && s.to_string().length() > 0);
        if on_control || selecting {
            return;
        }
        if ev.ctrl_key() || ev.meta_key() || ev.shift_key() {
            let _ = window().open_with_url_and_target(&href, "_blank");
            return;
        }
        // Straight to the router, like a link: a view transition here would
        // first snapshot the whole feed, which costs about 100 ms.
        navigate(&href, NavigateOptions::default());
    }
}

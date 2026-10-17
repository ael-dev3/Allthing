// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keyboard navigation: j/k move between casts and replies, o or Enter opens,
//! x collapses a reply. Ignored while typing or when the media viewer is open.

use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, ScrollBehavior, ScrollIntoViewOptions, ScrollLogicalPosition};

use crate::components::media::use_viewer;

fn items() -> Vec<Element> {
    let Ok(list) = document().query_selector_all("[data-nav]") else { return Vec::new() };
    (0..list.length())
        .filter_map(|i| list.get(i)?.dyn_into::<Element>().ok())
        // Skip replies hidden inside a collapsed parent.
        .filter(|e| e.closest(".comment.collapsed .comment-body").ok().flatten().is_none())
        .collect()
}

fn selected() -> Option<Element> {
    document().query_selector("[data-nav].selected").ok().flatten()
}

fn select(element: &Element) {
    if let Some(old) = selected() {
        let _ = old.class_list().remove_1("selected");
    }
    let _ = element.class_list().add_1("selected");
    let options = ScrollIntoViewOptions::new();
    options.set_block(ScrollLogicalPosition::Nearest);
    options.set_behavior(if crate::util::prefers_reduced_motion() {
        ScrollBehavior::Instant
    } else {
        ScrollBehavior::Smooth
    });
    element.scroll_into_view_with_scroll_into_view_options(&options);
}

fn click_in(element: &Element, selector: &str) {
    if let Some(target) = element.query_selector(selector).ok().flatten().and_then(|e| e.dyn_into::<HtmlElement>().ok())
    {
        target.click();
    }
}

#[component]
pub fn KeyboardNav() -> impl IntoView {
    let viewer = use_viewer();
    let handle = window_event_listener(ev::keydown, move |ev| {
        if ev.ctrl_key() || ev.meta_key() || ev.alt_key() || viewer.gallery.with_untracked(Option::is_some) {
            return;
        }
        let typing = ev.target().and_then(|t| t.dyn_into::<Element>().ok()).is_some_and(|t| {
            matches!(t.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT") || t.has_attribute("contenteditable")
        });
        if typing {
            return;
        }
        let list = items();
        let current = selected().and_then(|s| list.iter().position(|e| *e == s));
        match ev.key().as_str() {
            "j" | "k" => {
                let next = match (ev.key().as_str(), current) {
                    ("j", None) => 0,
                    ("j", Some(i)) => (i + 1).min(list.len().saturating_sub(1)),
                    (_, None) => 0,
                    (_, Some(i)) => i.saturating_sub(1),
                };
                if let Some(element) = list.get(next) {
                    ev.prevent_default();
                    select(element);
                }
            }
            "o" | "Enter" => {
                if let Some(element) = current.and_then(|i| list.get(i)) {
                    ev.prevent_default();
                    click_in(element, "[data-primary]");
                }
            }
            "x" => {
                if let Some(element) = current.and_then(|i| list.get(i)) {
                    ev.prevent_default();
                    click_in(element, ":scope > .comment-head > .toggle");
                }
            }
            "Escape" => {
                if let Some(old) = selected() {
                    let _ = old.class_list().remove_1("selected");
                }
            }
            _ => {}
        }
    });
    on_cleanup(move || handle.remove());

    // Clicking a cast or reply selects it, so keys continue from there.
    let clicks = window_event_listener(ev::click, move |ev| {
        if let Some(item) =
            ev.target().and_then(|t| t.dyn_into::<Element>().ok()).and_then(|t| t.closest("[data-nav]").ok().flatten())
            && !item.class_list().contains("selected")
        {
            if let Some(old) = selected() {
                let _ = old.class_list().remove_1("selected");
            }
            let _ = item.class_list().add_1("selected");
        }
    });
    on_cleanup(move || clicks.remove());
}

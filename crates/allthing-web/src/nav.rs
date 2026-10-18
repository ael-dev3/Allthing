// SPDX-License-Identifier: AGPL-3.0-or-later
//! Navigation that feels instant: scroll positions restored on back and
//! forward, and cards that open anywhere they are clicked (except on links,
//! controls, media, or selected text).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use leptos::ev;
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::{JsCast, JsValue};

thread_local! {
    /// Last scroll position of each visited page (pathname + query).
    static POSITIONS: RefCell<HashMap<String, Position>> = RefCell::new(HashMap::new());
    /// Set while a restore is in progress, so clamped scrolls aren't saved.
    static RESTORING: Cell<bool> = const { Cell::new(false) };
    /// Bumped by every restore and by the reader's own input; a restore
    /// loop stops when the generation it started with is no longer current.
    static GENERATION: Cell<u32> = const { Cell::new(0) };
    /// The page a click just left from, and when: its saved position is kept
    /// against the scrolls a navigation causes (the old page's DOM is
    /// replaced before the URL changes, and the browser clamps the scroll).
    static LEAVING: RefCell<Option<(String, f64)>> = const { RefCell::new(None) };
}

/// How long after a click scroll events for the same page are not saved.
const LEAVING_MS: f64 = 1000.0;

/// How long after Back the restored position is held against late layout
/// changes (lists growing, a late router scroll-to-top), unless the reader
/// scrolls first.
const HOLD_MS: f64 = 6000.0;

/// Where the reader was: the card at the top of the viewport and how far
/// from the top it sat (stable while images above it load), and the pixel
/// offset as a fallback for pages without cards.
#[derive(Clone)]
struct Position {
    y: f64,
    anchor: Option<(String, f64)>,
}

/// Viewport line a little under the sticky header, where the anchor card is read.
const ANCHOR_LINE: f64 = 120.0;

fn current_position() -> Position {
    Position { y: scroll_y(), anchor: anchor_card() }
}

/// The card crossing `ANCHOR_LINE`, and its top relative to the viewport.
fn anchor_card() -> Option<(String, f64)> {
    let doc = document();
    let list = doc.query_selector(".cast-list").ok().flatten()?;
    let x = list.get_bounding_client_rect().left() + 24.0;
    [ANCHOR_LINE, ANCHOR_LINE + 30.0, ANCHOR_LINE + 60.0].into_iter().find_map(|y| {
        let card = doc.element_from_point(x as f32, y as f32)?.closest(".cast-card[data-hash]").ok().flatten()?;
        Some((card.get_attribute("data-hash")?, card.get_bounding_client_rect().top()))
    })
}

/// Whether Back or Forward is putting the reader's place back right now.
pub fn restoring() -> bool {
    RESTORING.with(Cell::get)
}

/// The current page's key: pathname plus query.
pub fn location_key() -> String {
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
        if RESTORING.with(Cell::get) {
            return;
        }
        let key = location_key();
        let leaving = LEAVING.with(|l| {
            let mut l = l.borrow_mut();
            match l.as_ref() {
                Some((from, at)) if *from == key && js_sys::Date::now() - at < LEAVING_MS => true,
                _ => {
                    *l = None;
                    false
                }
            }
        });
        if !leaving {
            POSITIONS.with(|p| p.borrow_mut().insert(key, current_position()));
        }
    });
    // A click may navigate away: save the place now, in the capture phase,
    // before the router or a card handler starts moving anything.
    let click = wasm_bindgen::closure::Closure::<dyn Fn(web_sys::Event)>::new(|_: web_sys::Event| {
        if RESTORING.with(Cell::get) {
            return;
        }
        let key = location_key();
        POSITIONS.with(|p| p.borrow_mut().insert(key.clone(), current_position()));
        LEAVING.with(|l| *l.borrow_mut() = Some((key, js_sys::Date::now())));
    });
    let _ = window().add_event_listener_with_callback_and_bool("click", click.as_ref().unchecked_ref(), true);
    let click = StoredValue::new_local(click);
    let popstate = window_event_listener(ev::popstate, |_| {
        let Some(target) = POSITIONS.with(|p| p.borrow().get(&location_key()).cloned()) else { return };
        RESTORING.with(|r| r.set(true));
        let generation = GENERATION.with(|g| {
            g.set(g.get().wrapping_add(1));
            g.get()
        });
        let key = location_key();
        restore(target, key, generation, js_sys::Date::now());
    });
    // The reader taking over ends any restore in progress.
    let stop = |_| {
        if RESTORING.with(Cell::get) {
            GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
            RESTORING.with(|r| r.set(false));
        }
    };
    let inputs = [
        window_event_listener(ev::wheel, move |e| stop(e.type_())),
        window_event_listener(ev::touchstart, move |e| stop(e.type_())),
        window_event_listener(ev::keydown, move |e| stop(e.type_())),
        window_event_listener(ev::mousedown, move |e| stop(e.type_())),
    ];
    on_cleanup(move || {
        scroll.remove();
        click.with_value(|c| {
            let _ = window().remove_event_listener_with_callback_and_bool("click", c.as_ref().unchecked_ref(), true);
        });
        popstate.remove();
        for input in inputs {
            input.remove();
        }
    });
}

/// Bring the anchor card back to where it sat (or, without one, the pixel
/// offset once the page is tall enough), and keep it there for `HOLD_MS`:
/// content that arrives late (images above it, a list growing back, a late
/// scroll-to-top from the router) must not move the reader.
fn restore(target: Position, key: String, generation: u32, started: f64) {
    if GENERATION.with(Cell::get) != generation || location_key() != key {
        return;
    }
    let card = target.anchor.as_ref().and_then(|(hash, top)| {
        let selector = format!(".cast-card[data-hash=\"{hash}\"]");
        let element = document().query_selector(&selector).ok().flatten()?;
        Some((element, *top))
    });
    let placed = match card {
        Some((element, top)) => {
            let delta = element.get_bounding_client_rect().top() - top;
            if delta.abs() > 2.0 {
                window().scroll_to_with_x_and_y(0.0, scroll_y() + delta);
            }
            true
        }
        None => {
            let height = document().document_element().map(|e| f64::from(e.scroll_height())).unwrap_or(0.0);
            let viewport = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
            let reachable = height - viewport >= target.y - 1.0;
            if reachable && (scroll_y() - target.y).abs() > 2.0 {
                window().scroll_to_with_x_and_y(0.0, target.y);
            }
            reachable
        }
    };
    if js_sys::Date::now() - started >= HOLD_MS {
        if !placed {
            window().scroll_to_with_x_and_y(0.0, target.y);
        }
        RESTORING.with(|r| r.set(false));
        POSITIONS.with(|p| p.borrow_mut().insert(key, current_position()));
        return;
    }
    request_animation_frame(move || restore(target, key, generation, started));
}

/// Elements whose clicks belong to themselves, not to the card.
const INTERACTIVE: &str =
    "a, button, input, textarea, select, summary, label, video, iframe, img, .popover, .media-grid, [role=dialog]";

/// A click handler for a whole card: open `href` unless the click was on an
/// interactive element or ended a text selection. Ctrl/⌘-click opens a tab.
///
/// A click it opens is marked handled (`preventDefault`), so a card inside
/// a card (a quoted cast) opens its own post, and the enclosing card's
/// handler, which runs next, leaves it alone.
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
        ev.prevent_default();
        if ev.ctrl_key() || ev.meta_key() || ev.shift_key() {
            let _ = window().open_with_url_and_target(&href, "_blank");
            return;
        }
        // Straight to the router, like a link.
        navigate(&href, NavigateOptions::default());
    }
}

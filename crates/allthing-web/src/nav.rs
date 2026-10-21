// SPDX-License-Identifier: AGPL-3.0-or-later
//! Navigation that feels instant: links and cards that open their page
//! once the press has shown and the page has something to show, with the
//! page left on screen meanwhile; scroll positions restored on back and
//! forward; and cards that open anywhere they are clicked (except on links,
//! controls, media, or selected text).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use allthing_core::api::PageRead;
use leptos::ev;
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::api;
use crate::components::cast::after_paint;
use crate::util::{BASE, use_go};

/// How long a navigation keeps the page it leaves on screen, waiting for the
/// next page's data, when that page would have nothing to show without it.
/// Past this it goes anyway, and the next page shows its placeholders until
/// the data comes.
const READY_WAIT_MS: f64 = 250.0;

/// The router's navigation, as `use_go` gives it.
type Navigate = Rc<dyn Fn(&str, NavigateOptions)>;

thread_local! {
    static GO: RefCell<Option<Navigate>> = const { RefCell::new(None) };
    /// Bumped by every navigation `go` starts, and by Back and Forward: a
    /// navigation still waiting gives up when its own is no longer current.
    static GOING: Cell<u32> = const { Cell::new(0) };
    /// What the reader pressed for a navigation still waiting, shown pressed
    /// until it goes.
    static HELD: RefCell<Option<web_sys::Element>> = const { RefCell::new(None) };
}

/// What the page at `href` (a path in the app, with the static copy's base)
/// reads first.
pub fn page_read(href: &str) -> Option<PageRead> {
    let href = href.strip_prefix(BASE).unwrap_or(href);
    let href = href.split('#').next().unwrap_or(href);
    let (path, query) = href.split_once('?').unwrap_or((href, ""));
    allthing_core::api::page_read(path, query)
}

/// Whether the page `read` describes has something to show the moment it's
/// drawn: its data kept, or, for a post or a profile, what a page already
/// showed of it.
fn shows_at_once(read: &PageRead) -> bool {
    api::kept(&read.api)
        || read.cast.as_deref().is_some_and(|hash| api::seen_cast(hash).is_some())
        || read.user.as_deref().is_some_and(|name| api::seen_person(name).is_some())
}

/// Go to `href` (a path in the app, with the static copy's base) as a click
/// on a link to it does, but not in the click's own task: once the press has
/// painted, so the tap is answered at once, and, for a page that would have
/// nothing to show, once its data is in (it's fetched now, if it isn't on
/// its way) or `READY_WAIT_MS` have passed; for a page whose code is its own,
/// once that has come too (`split::page_came`). Meanwhile the page left
/// stays on screen, with `pressed` held pressed. A later navigation, or Back,
/// cancels one still waiting.
pub fn go(href: String, options: NavigateOptions, pressed: Option<web_sys::Element>) {
    let generation = GOING.with(|g| {
        g.set(g.get().wrapping_add(1));
        g.get()
    });
    let wait = page_read(&href).filter(|read| !shows_at_once(read)).map(|read| read.api);
    if let Some(path) = &wait {
        api::fetch_ahead(path);
    }
    crate::split::preload_page(&href);
    hold(pressed);
    let from = Started { generation, page: location_key(), since: js_sys::Date::now() };
    after_paint(move || when_ready(from, href, options, wait));
}

/// A navigation `go` started: which, from what page, and when.
struct Started {
    generation: u32,
    page: String,
    since: f64,
}

fn when_ready(from: Started, href: String, options: NavigateOptions, wait: Option<String>) {
    if GOING.with(Cell::get) != from.generation {
        return;
    }
    // The page changed some other way meanwhile (a search, a sort tab): the
    // reader has moved on.
    if location_key() != from.page {
        cancel();
        return;
    }
    // Its data, if it waits for that, and its code.
    let ready =
        wait.as_deref().is_none_or(|path| api::kept(path) || !api::loading(path)) && crate::split::page_came(&href);
    if !ready && js_sys::Date::now() - from.since < READY_WAIT_MS {
        request_animation_frame(move || when_ready(from, href, options, wait));
        return;
    }
    release_held();
    if let Some(go) = GO.with(|go| go.borrow().clone()) {
        go(&href, options);
    }
}

/// Back, Forward or another navigation: one still waiting doesn't go.
pub fn cancel() {
    GOING.with(|g| g.set(g.get().wrapping_add(1)));
    release_held();
}

fn hold(pressed: Option<web_sys::Element>) {
    release_held();
    if let Some(element) = pressed {
        let _ = element.set_attribute("data-press", "held");
        HELD.with(|held| *held.borrow_mut() = Some(element));
    }
}

fn release_held() {
    if let Some(element) = HELD.with(|held| held.borrow_mut().take())
        && element.get_attribute("data-press").as_deref() == Some("held")
    {
        let _ = element.remove_attribute("data-press");
    }
}

/// The app's links, opened as `go` opens a page: a click listener on the
/// document, which runs before the router's own on the window and leaves it
/// any click it doesn't take (a key held, a link with a target, a download,
/// another site, or the same page with another query, as a sort tab is,
/// which the router answers in the same task).
#[component]
pub fn InstantLinks() -> impl IntoView {
    let navigate = use_go();
    GO.with(|go| *go.borrow_mut() = Some(Rc::new(navigate)));
    let click = Closure::<dyn Fn(web_sys::MouseEvent)>::new(|ev: web_sys::MouseEvent| {
        if let Some((link, href, options)) = in_app_link(&ev) {
            ev.prevent_default();
            go(href, options, Some(link));
        }
    });
    let _ = document().add_event_listener_with_callback("click", click.as_ref().unchecked_ref());
    let click = StoredValue::new_local(click);
    let popstate = window_event_listener(ev::popstate, |_| cancel());
    on_cleanup(move || {
        click.with_value(|c| {
            let _ = document().remove_event_listener_with_callback("click", c.as_ref().unchecked_ref());
        });
        popstate.remove();
        GO.with(|go| go.borrow_mut().take());
    });
}

/// The link a click would have the router follow to another page of the
/// app, the path it goes to, and how.
fn in_app_link(ev: &web_sys::MouseEvent) -> Option<(web_sys::Element, String, NavigateOptions)> {
    if ev.default_prevented() || ev.button() != 0 || ev.meta_key() || ev.alt_key() || ev.ctrl_key() || ev.shift_key() {
        return None;
    }
    let link = ev.target()?.dyn_into::<web_sys::Element>().ok()?.closest("a[href]").ok()??;
    let external = link.get_attribute("rel").is_some_and(|rel| rel.split_whitespace().any(|r| r == "external"));
    if link.get_attribute("target").is_some_and(|t| !t.is_empty()) || link.has_attribute("download") || external {
        return None;
    }
    let location = window().location();
    let url = web_sys::Url::new_with_base(&link.get_attribute("href")?, &location.href().ok()?).ok()?;
    let path = url.pathname();
    if url.origin() != location.origin().ok()? || !path.starts_with(BASE) || path == location.pathname().ok()? {
        return None;
    }
    let replace = js_sys::Reflect::get(&link, &JsValue::from_str("replace")).ok().and_then(|r| r.as_bool());
    let options = NavigateOptions {
        replace: replace.unwrap_or(false),
        scroll: !link.has_attribute("noscroll") && !link.has_attribute("data-noscroll"),
        ..Default::default()
    };
    Some((link, format!("{path}{}{}", url.search(), url.hash()), options))
}

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
        Some((card.get_attribute("data-hash")?, laid_out_top(&card)))
    })
}

/// Where `card`'s box is laid out, from the top of the viewport: without its
/// own transform, which a pointer resting on it (a 1 px lift) or its
/// entrance (8 px) moves it by for a moment, so the place kept is the
/// card's, not where the pointer happened to leave it.
fn laid_out_top(card: &web_sys::Element) -> f64 {
    let lift = window()
        .get_computed_style(card)
        .ok()
        .flatten()
        .and_then(|style| style.get_property_value("transform").ok())
        .and_then(|matrix| translate_y(&matrix))
        .unwrap_or(0.0);
    card.get_bounding_client_rect().top() - lift
}

/// The vertical translation of a computed `transform` ("matrix(a, b, c, d,
/// x, y)"), if it has one.
fn translate_y(matrix: &str) -> Option<f64> {
    matrix.strip_prefix("matrix(")?.strip_suffix(')')?.split(',').nth(5)?.trim().parse().ok()
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
            let delta = laid_out_top(&element) - top;
            if delta.abs() >= 1.0 {
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
/// interactive element or ended a text selection, as `go` opens a page, the
/// card held pressed meanwhile. Ctrl/⌘-click opens a tab.
///
/// A click it opens is marked handled (`preventDefault`), so a card inside
/// a card (a quoted cast) opens its own post, and the enclosing card's
/// handler, which runs next, leaves it alone.
pub fn card_click(href: String) -> impl Fn(ev::MouseEvent) + Clone + 'static {
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
        let card = ev.current_target().and_then(|card| card.dyn_into::<web_sys::Element>().ok());
        go(href.clone(), NavigateOptions::default(), card);
    }
}

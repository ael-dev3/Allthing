// SPDX-License-Identifier: AGPL-3.0-or-later
//! The front page, kept while the reader reads another page: Back, or Home,
//! shows it at once and as it was, where drawing its 25 to 75 cards again
//! took 110 to 260 ms on the phone profile (docs/PERFORMANCE.md#smoothness).
//!
//! It's drawn the first time the front page shows, into an element of its
//! own (`.kept-front`, `display: contents`, first in `<main>`), and taken
//! out of the page while the address is another page's. Out of it, nothing
//! of it is laid out or painted, found by a selector or read by a screen
//! reader, and its videos pause, as anything taken out of a page does; its
//! popovers close (`popover.rs`). Meanwhile it reads the address it last
//! showed (its sort, time range and client), so another page doesn't redraw
//! it. Out for longer than the client cache keeps its data, it's drawn anew
//! when it shows again.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use leptos::prelude::*;
use leptos::tachys::view::{Mountable, Render};
use leptos_router::hooks::use_url;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::pages::home::Home;
use crate::util::BASE;

/// How long the front page is kept out of the page before it's drawn anew:
/// as long as the client cache keeps its data.
const KEEP_MS: f64 = 300_000.0;

/// What has an entrance on the front page: put back in the page, it would
/// play it again.
const ENTERED: &str = ".cast-card, .share-bar, .hidden-count, .poll-card";

struct Kept {
    /// What it was drawn under: cleaned up, it's disposed of.
    owner: Owner,
    view: Box<dyn Mountable>,
    /// When it was taken out of the page, while it is.
    out_since: Option<f64>,
}

thread_local! {
    static KEPT: RefCell<Option<Kept>> = const { RefCell::new(None) };
    static HEIGHT_OBSERVER: Option<HeightObserver> = height_observer();
    // Private keys leave hydrated card markup identical to early paint.
    static HEIGHT_IDS: js_sys::WeakMap = js_sys::WeakMap::new();
    static HEIGHTS: RefCell<HashMap<u64, ObservedCard>> = RefCell::new(HashMap::new());
    static NEXT_HEIGHT: Cell<u64> = const { Cell::new(0) };
}

// Opaque IDs connect weak element keys to Rust measurements without adding
// attributes or properties to cards. Only mounted Home cards register.
const SIZE_TOLERANCE: f64 = 0.000_001;

#[derive(Clone, Copy, Debug)]
struct ObservedHeight {
    content: f64,
    border_height: f64,
    border_width: f64,
}

impl ObservedHeight {
    fn new(content: f64, border_height: f64, border_width: f64) -> Option<Self> {
        [content, border_height, border_width]
            .into_iter()
            .all(|size| size.is_finite() && size > 0.0)
            .then_some(Self { content, border_height, border_width })
            .filter(|size| size.content <= size.border_height + SIZE_TOLERANCE)
    }

    fn for_box(self, height: f64, width: f64) -> Option<f64> {
        ((height - self.border_height).abs() <= SIZE_TOLERANCE && (width - self.border_width).abs() <= SIZE_TOLERANCE)
            .then_some(self.content)
    }
}

#[derive(Clone)]
struct ObservedCard {
    size: ObservedHeight,
    style: Option<String>,
    class: Option<String>,
}

struct HeightObserver {
    observer: web_sys::ResizeObserver,
    // The shared observer and its callback have the same lifetime.
    _callback: Closure<dyn Fn(js_sys::Array)>,
}

fn height_id(element: &web_sys::Element) -> Option<u64> {
    HEIGHT_IDS.with(|ids| ids.get_checked(element.unchecked_ref()).and_then(|id| id.as_string())?.parse().ok())
}

fn height_observer() -> Option<HeightObserver> {
    let callback = Closure::<dyn Fn(js_sys::Array)>::new(|entries: js_sys::Array| {
        for entry in entries.iter().filter_map(|entry| entry.dyn_into::<web_sys::ResizeObserverEntry>().ok()) {
            let element = entry.target();
            let Some(id) = height_id(&element).filter(|_| element.is_connected()) else { continue };
            // Missing/fragmented border boxes in older browsers keep the
            // existing exact measurement fallback. Reflect also tolerates a
            // browser that implements ResizeObserver without borderBoxSize.
            let border = js_sys::Reflect::get(&entry, &"borderBoxSize".into())
                .ok()
                .and_then(|value| value.dyn_into::<js_sys::Array>().ok())
                .filter(|boxes| boxes.length() == 1)
                .map(|boxes| boxes.get(0));
            let size = border.and_then(|border| {
                let height = js_sys::Reflect::get(&border, &"blockSize".into()).ok()?.as_f64()?;
                let width = js_sys::Reflect::get(&border, &"inlineSize".into()).ok()?.as_f64()?;
                ObservedHeight::new(entry.content_rect().height(), height, width)
            });
            HEIGHTS.with(|heights| {
                let mut heights = heights.borrow_mut();
                match size {
                    Some(size) => {
                        heights.insert(
                            id,
                            ObservedCard {
                                size,
                                style: element.get_attribute("style"),
                                class: element.get_attribute("class"),
                            },
                        );
                    }
                    None => {
                        heights.remove(&id);
                    }
                }
            });
        }
    });
    let observer = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
    Some(HeightObserver { observer, _callback: callback })
}

/// Remember a Home card's exact content height outside navigation, after
/// layout. Content-box observation follows the value we cache, including
/// content changes that leave the outer border box unchanged.
pub fn watch_height(element: &web_sys::Element) {
    HEIGHT_OBSERVER.with(|observer| {
        let Some(observer) = observer else { return };
        if height_id(element).is_some() {
            return;
        }
        let id = NEXT_HEIGHT.with(|next| {
            let id = next.get().wrapping_add(1);
            next.set(id);
            id
        });
        HEIGHT_IDS.with(|ids| ids.set(element.unchecked_ref(), &id.to_string().into()));
        observer.observer.observe(element);
    });
}

pub fn unwatch_height(element: &web_sys::Element) {
    HEIGHT_OBSERVER.with(|observer| {
        if let Some(observer) = observer {
            observer.observer.unobserve(element);
        }
    });
    if let Some(id) = height_id(element) {
        HEIGHTS.with(|heights| heights.borrow_mut().remove(&id));
    }
    HEIGHT_IDS.with(|ids| ids.delete(element.unchecked_ref()));
}

fn observed_height(element: &web_sys::HtmlElement) -> Option<f64> {
    let id = height_id(element)?;
    let observed = HEIGHTS.with(|heights| heights.borrow().get(&id).cloned())?;
    // A late edge change can leave the border box unchanged while the
    // observer's content-height delivery is still pending. In that case
    // keep the CSS fallback rather than trusting the old content height.
    let unchanged =
        observed.style == element.get_attribute("style") && observed.class == element.get_attribute("class");
    let held = unchanged
        .then(|| {
            // Rounded offsetHeight cannot detect a late fractional change. A
            // single exact box checks both dimensions and rejects transforms
            // and responsive width changes alike.
            let rect = element.get_bounding_client_rect();
            observed.size.for_box(rect.height(), rect.width())
        })
        .flatten();
    if held.is_none() {
        HEIGHTS.with(|heights| heights.borrow_mut().remove(&id));
    }
    held
}

fn remember_retention_style(element: &web_sys::HtmlElement) {
    let Some(id) = height_id(element) else { return };
    HEIGHTS.with(|heights| {
        if let Some(observed) = heights.borrow_mut().get_mut(&id) {
            // Only entries accepted above survive. Our own two style
            // writes must not make their next reuse look like a late edge
            // change, nor make an invalidated entry trustworthy again.
            observed.style = element.get_attribute("style");
            observed.class = element.get_attribute("class");
        }
    });
}

/// Whether `path` is the front page's.
pub fn is_front(path: &str) -> bool {
    let path = path.strip_prefix(BASE).unwrap_or(path);
    path == "/" || path.is_empty()
}

/// The front page, kept (see the module). Its route draws nothing.
#[component]
pub fn KeptFront() -> impl IntoView {
    let url = use_url();
    let shown = Memo::new(move |_| url.with(|url| is_front(url.path())));
    // What the front page reads for its address: the router's while it
    // shows, the last it showed while it's out.
    let address = ArcRwSignal::new(url.get_untracked());
    provide_context(address.clone());
    let slot = NodeRef::<leptos::html::Div>::new();
    let parent = Owner::current().expect("an owner");
    Effect::new(move |_| {
        let Some(slot) = slot.get() else { return };
        if !shown.get() {
            take_out(&slot);
            return;
        }
        let now = url.get();
        if address.with_untracked(|kept| *kept != now) {
            address.set(now);
        }
        put_in(&parent, &slot);
    });
    on_cleanup(|| {
        if let Some(kept) = KEPT.with(|kept| kept.borrow_mut().take()) {
            kept.owner.cleanup();
        }
    });
    view! { <div class="kept-front" node_ref=slot></div> }
}

/// Show the front page: as it was, or drawn anew if it hasn't been yet or
/// has been out too long.
fn put_in(parent: &Owner, slot: &web_sys::HtmlDivElement) {
    let now = js_sys::Date::now();
    let stale = KEPT
        .with(|kept| kept.borrow().as_ref().is_some_and(|kept| kept.out_since.is_some_and(|out| now - out > KEEP_MS)));
    if stale && let Some(kept) = KEPT.with(|kept| kept.borrow_mut().take()) {
        kept.owner.cleanup();
    }
    let drawn = KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        let Some(kept) = kept.as_mut() else { return false };
        if kept.out_since.take().is_some() {
            kept.view.mount(slot, None);
        }
        true
    });
    if !drawn {
        let owner = parent.child();
        // Drawn under an owner of its own, and not as part of this effect,
        // which mustn't run again for what the page reads.
        let mut view: Box<dyn Mountable> = Box::new(owner.with(|| untrack(|| view! { <Home/> }.into_any().build())));
        view.mount(slot, None);
        KEPT.with(|kept| *kept.borrow_mut() = Some(Kept { owner, view, out_since: None }));
    }
}

/// Take the front page out of the page, kept.
fn take_out(slot: &web_sys::HtmlDivElement) {
    let taken = KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        let Some(kept) = kept.as_mut().filter(|kept| kept.out_since.is_none()) else { return false };
        // Every height read comes before animation or retention writes,
        // so those writes cannot force another style/layout calculation.
        let heights = measure_heights(slot);
        // What has come in stays in: put back, it would come in again.
        if let Ok(entered) = slot.query_selector_all(ENTERED) {
            for i in 0..entered.length() {
                if let Some(element) = entered.item(i).and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok()) {
                    let _ = element.style().set_property("animation-name", "none");
                }
            }
        }
        for (card, held) in heights {
            let _ = card.style().set_property("contain-intrinsic-height", &format!("auto {held}px"));
            remember_retention_style(&card);
        }
        kept.view.unmount();
        kept.out_since = Some(js_sys::Date::now());
        true
    });
    // Its first cards, drawn before the app came, can't stay over the next
    // page.
    if taken {
        crate::boot::hand_over();
    }
}

/// Each card in `slot`, and the poll in its feed, keeps the height it has.
/// A card out of view is laid out at the height it last had in view,
/// without laying out what it holds (`content-visibility: auto`), and it
/// forgets that height out of the page:
/// put back, each card not yet back in view was laid out as if it held
/// 180 px, so a front page of 30 cards came back 2,800 px taller at 1280 px,
/// and moved under the reader as its cards were laid out again, one screen
/// at a time. With the heights kept, it's as it was from its first frame.
fn measure_heights(slot: &web_sys::Element) -> Vec<(web_sys::HtmlElement, f64)> {
    let Ok(cards) = slot.query_selector_all(".cast-card, .cast-list > .poll-card") else { return Vec::new() };
    (0..cards.length())
        .filter_map(|i| {
            let card = cards.item(i)?.dyn_into::<web_sys::HtmlElement>().ok()?;
            if let Some(held) = observed_height(&card) {
                return Some((card, held));
            }
            let style = window().get_computed_style(&card).ok()??;
            let px = |property: &str| {
                let value = style.get_property_value(property).ok()?;
                value.strip_suffix("px")?.parse::<f64>().ok()
            };
            // Its height as laid out: the style's is written to six figures
            // ("133.359px" for 133.359375), which kept is laid out a 64th of
            // a pixel short, and over a page of cards that came back a pixel
            // shorter. Its box's, unless a transform scales it (a press).
            let style_height = px("height")?;
            let laid_out = card.get_bounding_client_rect().height();
            let height = if (laid_out - style_height).abs() < 0.01 { laid_out } else { style_height };
            // The height of what it holds, which the size it's laid out at
            // stands for: its height less its padding and borders.
            let held = ["padding-top", "padding-bottom", "border-top-width", "border-bottom-width"]
                .into_iter()
                .try_fold(height, |left, edge| Some(left - px(edge)?))?;
            (held > 0.0).then_some((card, held))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::ObservedHeight;

    #[test]
    fn observed_sizes_preserve_fractional_heights_and_require_both_exact_dimensions() {
        let size = ObservedHeight::new(308.125, 350.125, 770.375).unwrap();
        assert_eq!(size.for_box(350.125, 770.375), Some(308.125));
        assert_eq!(size.for_box(350.375, 770.375), None, "same rounded offsetHeight is not enough");
        assert_eq!(size.for_box(350.125, 770.625), None, "a responsive width change invalidates it");
        assert_eq!(size.for_box(350.125 * 0.96, 770.375 * 0.96), None, "transforms retain the fallback");
        assert_eq!(size.for_box(f64::NAN, 770.375), None);
    }

    #[test]
    fn empty_unmounted_and_invalid_observations_are_not_kept() {
        for (content, height, width) in [
            (0.0, 0.0, 0.0),
            (100.0, 0.0, 200.0),
            (100.0, 120.0, 0.0),
            (121.0, 120.0, 200.0),
            (f64::NAN, 120.0, 200.0),
            (100.0, f64::INFINITY, 200.0),
        ] {
            assert!(ObservedHeight::new(content, height, width).is_none());
        }
    }
}

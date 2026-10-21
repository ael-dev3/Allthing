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

use std::cell::RefCell;

use leptos::prelude::*;
use leptos::tachys::view::{Mountable, Render};
use leptos_router::hooks::use_url;
use wasm_bindgen::JsCast;

use crate::pages::home::Home;
use crate::util::BASE;

/// How long the front page is kept out of the page before it's drawn anew:
/// as long as the client cache keeps its data.
const KEEP_MS: f64 = 300_000.0;

/// What has an entrance on the front page: put back in the page, it would
/// play it again.
const ENTERED: &str = ".cast-card, .share-bar, .hidden-count";

struct Kept {
    /// What it was drawn under: cleaned up, it's disposed of.
    owner: Owner,
    view: Box<dyn Mountable>,
    /// When it was taken out of the page, while it is.
    out_since: Option<f64>,
}

thread_local! {
    static KEPT: RefCell<Option<Kept>> = const { RefCell::new(None) };
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
        // What has come in stays in: put back, it would come in again.
        if let Ok(entered) = slot.query_selector_all(ENTERED) {
            for i in 0..entered.length() {
                if let Some(element) = entered.item(i).and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok()) {
                    let _ = element.style().set_property("animation-name", "none");
                }
            }
        }
        keep_heights(slot);
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

/// Each card in `slot` keeps the height it has. A card out of view is laid
/// out at the height it last had in view, without laying out what it holds
/// (`content-visibility: auto`), and it forgets that height out of the page:
/// put back, each card not yet back in view was laid out as if it held
/// 180 px, so a front page of 30 cards came back 2,800 px taller at 1280 px,
/// and moved under the reader as its cards were laid out again, one screen
/// at a time. With the heights kept, it's as it was from its first frame.
fn keep_heights(slot: &web_sys::Element) {
    let Ok(cards) = slot.query_selector_all(".cast-card") else { return };
    // All read, then all kept: a height kept between two reads would have
    // the page laid out again for the next.
    let heights: Vec<_> = (0..cards.length())
        .filter_map(|i| {
            let card = cards.item(i)?.dyn_into::<web_sys::HtmlElement>().ok()?;
            let style = window().get_computed_style(&card).ok()??;
            let px = |property: &str| {
                let value = style.get_property_value(property).ok()?;
                value.strip_suffix("px")?.parse::<f64>().ok()
            };
            // The height of what it holds, which the size it's laid out at
            // stands for: its height less its padding and borders.
            let held = ["padding-top", "padding-bottom", "border-top-width", "border-bottom-width"]
                .into_iter()
                .try_fold(px("height")?, |left, edge| Some(left - px(edge)?))?;
            (held > 0.0).then_some((card, held))
        })
        .collect();
    for (card, held) in heights {
        let _ = card.style().set_property("contain-intrinsic-height", &format!("auto {held}px"));
    }
}

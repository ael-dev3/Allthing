// SPDX-License-Identifier: AGPL-3.0-or-later
//! The boot screen (index.html's `#boot`), and the front page's first cards
//! web/paint.js may have drawn in it from the early fetch while the app was
//! still on its way. Without them, the app takes the boot screen's place as
//! soon as it's mounted. With them, the app is drawn hidden under the boot
//! screen, and takes its place once its own cards are drawn (`hand_over`):
//! in the same places, so nothing moves, and without their entrance again.
//! A front page that can't draw them (its data didn't come), or leaving it,
//! hands over too, and so does a wait of a few seconds, whatever else.

use std::time::Duration;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::util::document;

/// Longest the boot screen's cards stay after the app is mounted.
const LONGEST: Duration = Duration::from_secs(4);

fn boot() -> Option<web_sys::Element> {
    document().get_element_by_id("boot")
}

/// The app is mounted: the boot screen goes, unless it holds cards, which
/// stay until the app's own are drawn.
pub fn mounted() {
    match boot() {
        Some(boot) if boot.has_attribute("data-painted") => set_timeout(hand_over, LONGEST),
        Some(boot) => boot.remove(),
        None => {}
    }
}

/// Show the app in the boot screen's place. What it drew shows as it is,
/// with no second entrance (the page's, the usage bar's and the cards'),
/// and its images at the sizes the boot screen's learned (see
/// `media::known_size`).
pub fn hand_over() {
    let Some(boot) = boot() else { return };
    if boot.has_attribute("data-painted")
        && let Some(app) = document().get_element_by_id("app")
    {
        let painted: Vec<String> =
            all(&boot, ".cast-card[data-hash]").filter_map(|card| card.get_attribute("data-hash")).collect();
        let drawn = all(&app, ".cast-card[data-hash]")
            .filter(|card| card.get_attribute("data-hash").is_some_and(|hash| painted.contains(&hash)));
        for entered in all(&app, ".page > *, .share-bar").chain(drawn) {
            if let Ok(entered) = entered.dyn_into::<web_sys::HtmlElement>() {
                let _ = entered.style().set_property("animation-name", "none");
            }
        }
        crate::components::media::size_tiles(&app);
    }
    boot.remove();
}

fn all(root: &web_sys::Element, selector: &str) -> impl Iterator<Item = web_sys::Element> {
    let found = root.query_selector_all(selector).ok();
    let count = found.as_ref().map_or(0, |list| list.length());
    (0..count).filter_map(move |i| found.as_ref()?.item(i)?.dyn_into::<web_sys::Element>().ok())
}

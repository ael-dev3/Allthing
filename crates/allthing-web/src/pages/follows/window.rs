// SPDX-License-Identifier: AGPL-3.0-or-later
//! A long list drawn a window at a time: of its rows, all as tall, only
//! those in view and [`OVERSCAN`] either side are in the page, placed where
//! they belong in a list as tall as all of them, so nothing shifts as the
//! window moves with the page's scroll. Thousands of people stay a few
//! dozen elements.

use leptos::html::Ol;
use leptos::prelude::*;

/// Rows drawn beyond the view, above and below it.
pub const OVERSCAN: usize = 8;
/// A row's height, in pixels, on wider screens and up to 640 px wide (as
/// `.person-row` in style/allthing.css).
const ROW_PX: f64 = 64.0;
const NARROW_ROW_PX: f64 = 72.0;
const NARROW: &str = "(max-width: 640px)";

/// Which rows are drawn, and how tall each is.
#[derive(Clone, Copy)]
pub struct Window {
    /// The rows drawn: from the first to before the last.
    pub range: RwSignal<(usize, usize)>,
    /// A row's height in pixels.
    pub row: RwSignal<f64>,
    /// How many rows a screen holds.
    pub screen: RwSignal<usize>,
}

fn row_px() -> f64 {
    let narrow = web_sys::window().and_then(|w| w.match_media(NARROW).ok().flatten()).is_some_and(|m| m.matches());
    if narrow { NARROW_ROW_PX } else { ROW_PX }
}

fn viewport() -> f64 {
    web_sys::window().and_then(|w| w.inner_height().ok()).and_then(|h| h.as_f64()).unwrap_or(800.0)
}

impl Window {
    /// The window over `list`, a list of `count` rows, following the
    /// page's scroll and size.
    pub fn new(list: NodeRef<Ol>, count: Signal<usize>) -> Window {
        let row = row_px();
        let screen = (viewport() / row).ceil() as usize;
        // Before the list is measured: as if it started at the top.
        let first = (screen + OVERSCAN).min(count.get_untracked());
        let window =
            Window { range: RwSignal::new((0, first)), row: RwSignal::new(row), screen: RwSignal::new(screen) };
        let measure = move || {
            let Some(list) = list.get_untracked() else { return };
            let row = window.row.get_untracked();
            let top = list.get_bounding_client_rect().top();
            let height = viewport();
            let n = count.get_untracked();
            let first = ((-top) / row).floor().max(0.0) as usize;
            let last = ((height - top) / row).ceil().max(0.0) as usize;
            let range = (first.saturating_sub(OVERSCAN).min(n), last.saturating_add(OVERSCAN).min(n));
            if window.range.get_untracked() != range {
                window.range.set(range);
            }
            let screen = (height / row).ceil() as usize;
            if window.screen.get_untracked() != screen {
                window.screen.set(screen);
            }
        };
        // Once a frame at most, however many scroll events come.
        let pending = StoredValue::new(false);
        let schedule = move || {
            if pending.get_value() {
                return;
            }
            pending.set_value(true);
            request_animation_frame(move || {
                // The page went meanwhile.
                if pending.try_set_value(false).is_some() {
                    return;
                }
                measure();
            });
        };
        let scroll = window_event_listener(leptos::ev::scroll, move |_| schedule());
        let resize = window_event_listener(leptos::ev::resize, move |_| {
            let row = row_px();
            if window.row.get_untracked() != row {
                window.row.set(row);
            }
            schedule();
        });
        on_cleanup(move || {
            scroll.remove();
            resize.remove();
        });
        // As the list mounts, grows or changes height; and again before the
        // next frame is drawn, by when Back has put the page's scroll back
        // (`crate::nav`), so that frame has the rows Back shows.
        Effect::new(move |_| {
            count.track();
            window.row.track();
            if list.get().is_some() {
                measure();
                schedule();
            }
        });
        window
    }
}

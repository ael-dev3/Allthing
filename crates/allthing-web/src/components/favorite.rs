// SPDX-License-Identifier: AGPL-3.0-or-later
//! Favorites (`crate::leanings`): the star on a profile that makes the
//! account one of the reader's favorites, in this browser, and the small
//! star their casts carry in the reader's feeds, where they rise.

use std::time::Duration;

use leptos::prelude::*;

use crate::leanings::use_leaning;

/// A five-pointed star.
const STAR: &str = "M8 1.9l1.85 3.8 4.15.6-3 2.95.7 4.15L8 11.45 4.3 13.4l.7-4.15-3-2.95 4.15-.6z";

/// How long a hint stays after a press.
const HINT_FOR: Duration = Duration::from_secs(3);

/// What Favorite or Show less does, under it: while the pointer rests on it
/// or keyboard focus is on it, and for a few seconds after a press, as a
/// phone can't hover. It's the button's description too. No native tooltip:
/// one shows late, in the system's style, and never on a phone.
#[derive(Clone, Copy)]
pub struct Hint {
    shown: RwSignal<bool>,
    presses: RwSignal<u32>,
}

impl Hint {
    pub fn new() -> Hint {
        Hint { shown: RwSignal::new(false), presses: RwSignal::new(0) }
    }

    /// Show it for a while from now, however many presses came before.
    pub fn flash(self) {
        let press = self.presses.get_untracked().wrapping_add(1);
        self.presses.set(press);
        self.shown.set(true);
        set_timeout(
            move || {
                if self.presses.try_get_untracked() == Some(press) {
                    let _ = self.shown.try_set(false);
                }
            },
            HINT_FOR,
        );
    }

    pub fn view(self, id: &'static str, text: &'static str) -> impl IntoView {
        view! { <span id=id class="leaning-hint" class:shown=move || self.shown.get()>{text}</span> }
    }
}

impl Default for Hint {
    fn default() -> Self {
        Hint::new()
    }
}

/// Favorite, at the top of a profile: the reader's own, kept in this
/// browser.
#[component]
pub fn FavoriteButton(fid: u64, username: String) -> impl IntoView {
    let leaning = use_leaning();
    let on = Memo::new(move |_| leaning.is_favorite(fid));
    let hint = Hint::new();
    view! {
        <span class="leaning">
            <button
                type="button"
                class="favorite-button"
                class:on=on
                aria-pressed=move || on.get().to_string()
                aria-describedby="favorite-what"
                on:click=move |_| {
                    leaning.toggle_favorite(fid, &username);
                    hint.flash();
                }
            >
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=STAR/></svg>
                "Favorite"
            </button>
            {hint.view("favorite-what", "Their casts rise in your hot and top feeds. Kept in this browser only.")}
        </span>
    }
}

/// The star a favorite's cast carries in the reader's feeds.
#[component]
pub fn FavoriteMark(fid: u64) -> impl IntoView {
    let leaning = use_leaning();
    move || {
        leaning.is_favorite(fid).then(|| {
            view! {
                <span class="favorite-mark" role="img" aria-label="One of your favorites">
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=STAR/></svg>
                </span>
            }
        })
    }
}

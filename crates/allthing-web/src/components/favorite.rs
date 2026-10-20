// SPDX-License-Identifier: AGPL-3.0-or-later
//! Favorites (`crate::leanings`): the star on a profile that makes the
//! account one of the reader's favorites, in this browser, and the small
//! star their casts carry in the reader's feeds, where they rise.

use leptos::prelude::*;

use crate::leanings::use_leaning;

/// A five-pointed star.
const STAR: &str = "M8 1.9l1.85 3.8 4.15.6-3 2.95.7 4.15L8 11.45 4.3 13.4l.7-4.15-3-2.95 4.15-.6z";

/// Favorite, at the top of a profile: the reader's own, kept in this
/// browser.
#[component]
pub fn FavoriteButton(fid: u64, username: String) -> impl IntoView {
    let leaning = use_leaning();
    let on = Memo::new(move |_| leaning.is_favorite(fid));
    view! {
        <button
            type="button"
            class="favorite-button"
            class:on=on
            aria-pressed=move || on.get().to_string()
            aria-describedby="favorite-what"
            title=move || if on.get() { "One of your favorites: click to stop" } else { "Make them a favorite: their casts rise in your feeds" }
            on:click=move |_| leaning.toggle_favorite(fid, &username)
        >
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=STAR/></svg>
            "Favorite"
        </button>
        <span id="favorite-what" class="visually-hidden">
            "Their casts rise in your hot and top feeds. Kept in this browser only."
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

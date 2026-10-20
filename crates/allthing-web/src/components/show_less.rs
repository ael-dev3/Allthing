// SPDX-License-Identifier: AGPL-3.0-or-later
//! Show less (`crate::leanings`): on a profile, the reader sees less of the
//! account, in this browser. Its casts sit a little lower in the reader's hot
//! and top feeds; they carry no mark, and nothing is hidden.

use leptos::prelude::*;

use crate::leanings::use_leaning;

/// A line trending down: lower, not gone.
const LOWER: &str = "M1.5 4.5L6 9l3-3 5.5 5.5M14.5 7.5v4h-4";

/// Show less, beside Favorite at the top of a profile: the reader's own,
/// kept in this browser.
#[component]
pub fn ShowLessButton(fid: u64, username: String) -> impl IntoView {
    let leaning = use_leaning();
    let on = Memo::new(move |_| leaning.shows_less(fid));
    view! {
        <button
            type="button"
            class="show-less-button"
            class:on=on
            aria-pressed=move || on.get().to_string()
            aria-describedby="show-less-what"
            title=move || if on.get() { "You see less of them: click to undo" } else { "See less of them: their casts sit a little lower in your feeds" }
            on:click=move |_| leaning.toggle_less(fid, &username)
        >
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=LOWER/></svg>
            "Show less"
        </button>
        <span id="show-less-what" class="visually-hidden">
            "Their casts sit a little lower in your hot and top feeds. Nothing is hidden. Kept in this browser only."
        </span>
    }
}

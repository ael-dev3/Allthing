// SPDX-License-Identifier: AGPL-3.0-or-later
//! Show less (`crate::leanings`): on a profile, the reader sees much less of
//! the account, in this browser. Stable sampling omits about 90% of their
//! feed posts; remaining hot/top candidates rank lower, while New retains
//! time order (`allthing_core::ranking::sorted_for`). Profiles stay available.

use leptos::prelude::*;

use crate::components::favorite::Hint;
use crate::leanings::use_leaning;

/// A line trending down: lower, not gone.
const LOWER: &str = "M1.5 4.5L6 9l3-3 5.5 5.5M14.5 7.5v4h-4";

/// Show less, beside Favorite at the top of a profile: the reader's own,
/// kept in this browser. In a profile card, `on_press` instead of the hint:
/// the card says what a press did.
#[component]
pub fn ShowLessButton(
    fid: u64,
    username: String,
    /// Called after a press, in place of the hint.
    #[prop(optional)]
    on_press: Option<Callback<()>>,
) -> impl IntoView {
    let leaning = use_leaning();
    let on = Memo::new(move |_| leaning.shows_less(fid));
    let hint = Hint::new();
    view! {
        <span class="leaning">
            <button
                type="button"
                class="show-less-button"
                class:on=on
                aria-pressed=move || on.get().to_string()
                aria-describedby=on_press.is_none().then_some("show-less-what")
                on:click=move |_| {
                    leaning.toggle_less(fid, &username);
                    match on_press {
                        Some(pressed) => pressed.run(()),
                        None => hint.flash(),
                    }
                }
            >
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=LOWER/></svg>
                "Show less"
            </button>
            {on_press.is_none().then(|| {
                hint.view(
                    "show-less-what",
                    "Omits about 90% of their posts from Home, Following and channel feeds. The rest rank much lower in hot and top; new stays chronological. Profiles stay available. Toggle again to undo. Kept in this browser only.",
                )
            })}
        </span>
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! One person in a followers or following list: their picture, name and
//! handle, the start of their bio, and how many follow them. The whole row
//! opens their profile, as a feed card opens its post (`nav::card_click`);
//! the name is the row's link, and pointing at it, keyboard focus resting
//! on it, or a tap on a phone shows their profile card. What a row may
//! add later (a Follow button) goes after the bio, beside the link, never
//! inside it.

use std::sync::Arc;

use allthing_core::api::Person;
use allthing_core::format::compact_number;
use leptos::prelude::*;

use crate::components::chips::Avatar;
use crate::components::profile_card::ProfileHover;
use crate::nav::card_click;
use crate::util::user_href;

/// The row of `person`, the `index`th of a list of `setsize` (`-1` while
/// that isn't known), `row` px tall, at its place in the list.
#[component]
pub fn PersonRow(person: Arc<Person>, index: usize, row: RwSignal<f64>, setsize: Signal<i64>) -> impl IntoView {
    let href = user_href(&person.username);
    let open = card_click(href.clone());
    let followers = person.followers;
    let bio = person.bio.clone();
    let (name, username) = (person.name().to_owned(), person.username.clone());
    let link = href.clone();
    view! {
        <li
            class="person-row"
            data-fid=person.fid.to_string()
            style:top=move || format!("{}px", index as f64 * row.get())
            aria-posinset=(index + 1).to_string()
            aria-setsize=move || setsize.get().to_string()
            on:click=open
        >
            <a class="person-row-pic" href=href tabindex="-1" aria-hidden="true">
                <Avatar url=person.pfp_url.clone() size=40/>
            </a>
            <ProfileHover user=person.summary() card=Some(person.card()) class="person-row-who">
                <a class="person-row-id" href=link.clone()>
                    <strong class="person-row-name">{name.clone()}</strong>
                    <span class="person-row-handle">"@"{username.clone()}</span>
                </a>
            </ProfileHover>
            <span class="person-row-bio">{bio}</span>
            <span class="person-row-stat">
                <strong>{compact_number(followers)}</strong>
                {if followers == 1 { " follower" } else { " followers" }}
            </span>
        </li>
    }
}

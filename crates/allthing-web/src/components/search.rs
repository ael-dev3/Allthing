// SPDX-License-Identifier: AGPL-3.0-or-later
//! The header's search box: one query for casts, people and channels.

use leptos::ev;
use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate, use_query_map};

use crate::util::{href, with_query};

#[component]
pub fn SearchBox() -> impl IntoView {
    let query = use_query_map();
    let location = use_location();
    let input = NodeRef::<leptos::html::Input>::new();
    let navigate = use_navigate();
    // Show the current query while on the results page.
    let current = move || {
        location.pathname.get().ends_with("/search").then(|| query.read().get("q")).flatten().unwrap_or_default()
    };
    let submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let q = input.get().map(|i| i.value()).unwrap_or_default();
        let q = q.trim();
        if !q.is_empty() {
            navigate(&with_query(&href("/search"), &[("q", Some(q.to_owned()))]), Default::default());
        }
    };
    view! {
        <form class="search-box" role="search" action=href("/search") method="get" on:submit=submit>
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5l3 3"/></svg>
            <input
                node_ref=input
                type="search"
                name="q"
                placeholder="Search"
                aria-label="Search casts, people and channels"
                autocomplete="off"
                enterkeyhint="search"
                maxlength="100"
                prop:value=current
            />
        </form>
    }
}

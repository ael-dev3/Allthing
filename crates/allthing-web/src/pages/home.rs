// SPDX-License-Identifier: AGPL-3.0-or-later
//! The front page: trending casts, ranked locally.

use std::sync::Arc;

use allthing_core::api::{HomePayload, home_page_path, home_path};
use allthing_core::listing::top_level;
use allthing_core::model::Cast;
use allthing_core::ranking::{Sort, sorted};
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::api::{self, Fetched};
use crate::components::cast::{CastList, CastSkeletons};
use crate::components::layout::{Columns, ErrorPanel, Freshness, SortTabs};
use crate::pages::side::ChannelsBox;
use crate::util::{href, set_title};

/// The trending list ends after this many casts.
const LAST_OFFSET: u32 = 900;
/// How long the later pages are kept for a return to the front page
/// (as long as the client cache keeps the first page).
const KEEP_MS: f64 = 300_000.0;

thread_local! {
    /// Later trending pages already loaded, kept for Back: (when, casts, next offset).
    static LOADED: std::cell::RefCell<Option<(f64, Vec<Cast>, u32)>> = const { std::cell::RefCell::new(None) };
}

#[component]
pub fn Home() -> impl IntoView {
    let query = use_query_map();
    let sort = Memo::new(move |_| Sort::parse(query.read().get("sort").as_deref(), Sort::Hot));
    let data = LocalResource::new(|| async { api::get::<HomePayload>(&home_path()).await });
    // Later trending pages, appended by infinite scroll (each ranked on its own).
    let (kept, kept_offset) = LOADED
        .with(|l| l.borrow().clone())
        .filter(|(at, _, _)| js_sys::Date::now() - at < KEEP_MS)
        .map(|(_, casts, offset)| (casts, offset))
        .unwrap_or((Vec::new(), 100));
    let more = RwSignal::new(kept);
    let next_offset = RwSignal::new(kept_offset);
    let loading = RwSignal::new(false);
    Effect::new(move |_| set_title(if sort.get() == Sort::Hot { "" } else { sort.get().as_str() }));
    let load_more = Callback::new(move |()| {
        let offset = next_offset.get_untracked();
        if loading.get_untracked() || offset > LAST_OFFSET {
            return;
        }
        loading.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(page) = api::get::<HomePayload>(&home_page_path(offset)).await {
                let known: std::collections::HashSet<String> =
                    more.with_untracked(|m| m.iter().map(|c| c.hash.clone()).collect());
                let fresh: Vec<Cast> = sorted(&top_level(&page.data.pool), sort.get_untracked())
                    .into_iter()
                    .filter(|c| !known.contains(&c.hash))
                    .collect();
                more.update(|m| m.extend(fresh));
                next_offset.set(offset + 100);
                LOADED.with(|l| {
                    *l.borrow_mut() = Some((js_sys::Date::now(), more.get_untracked(), offset + 100));
                });
            } else {
                next_offset.set(LAST_OFFSET + 100);
            }
            loading.set(false);
        });
    });

    move || {
        let side = |channels: Vec<allthing_core::model::Channel>| view! { <ChannelsBox channels/> }.into_any();
        match data.get() {
            None => view! {
                <Columns side=side(Vec::new())>
                    <Heading sort/>
                    <CastSkeletons/>
                </Columns>
            }
            .into_any(),
            Some(Err(failure)) => view! { <ErrorPanel failure/> }.into_any(),
            Some(Ok(Fetched { data, age, stale })) => {
                let payload: Arc<HomePayload> = data;
                let notes = Arc::new(payload.notes.clone());
                let pool = top_level(&payload.pool);
                let first_hashes: std::collections::HashSet<String> = pool.iter().map(|c| c.hash.clone()).collect();
                // Re-sorting reorders the list in place instead of rebuilding the page.
                let casts = Signal::derive(move || {
                    let mut all = sorted(&pool, sort.get());
                    more.with(|m| all.extend(m.iter().filter(|c| !first_hashes.contains(&c.hash)).cloned()));
                    all
                });
                view! {
                    <Columns side=side(payload.channels.clone())>
                        <Heading sort/>
                        <Freshness age stale/>
                        <CastList casts notes on_end=load_more/>
                        <Show when=move || loading.get()><CastSkeletons count=2/></Show>
                    </Columns>
                }
                .into_any()
            }
        }
    }
}

#[component]
fn Heading(sort: Memo<Sort>) -> impl IntoView {
    view! {
        <div class="listing-head">
            <h1 class="visually-hidden">"Trending on Farcaster"</h1>
            {move || view! { <SortTabs path=href("/") current=sort.get()/> }}
        </div>
    }
}

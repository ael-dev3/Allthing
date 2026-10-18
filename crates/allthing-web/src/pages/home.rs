// SPDX-License-Identifier: AGPL-3.0-or-later
//! The front page: trending casts, ranked locally.

use std::sync::Arc;

use allthing_core::api::{Annotations, HomePayload, home_page_path, home_path};
use allthing_core::listing::top_level;
use allthing_core::model::Cast;
use allthing_core::ranking::{Sort, sorted};
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::api::{self, Fetched};
use crate::components::cast::{CastList, CastSkeletons};
use crate::components::layout::{Columns, ErrorPanel, SortTabs, StaleNotice};
use crate::pages::side::ChannelsBox;
use crate::util::{href, set_title};

/// The trending list ends after this many casts.
const LAST_OFFSET: u32 = 900;
/// How many times a page that failed to load is tried again.
const MAX_RETRIES: u32 = 3;
/// How long the later pages are kept for a return to the front page
/// (as long as the client cache keeps the first page).
const KEEP_MS: f64 = 300_000.0;

#[derive(Clone)]
struct LoadedPage {
    casts: Vec<Cast>,
    notes: Annotations,
}

#[derive(Clone)]
struct Loaded {
    at: f64,
    pages: Vec<LoadedPage>,
    next_offset: u32,
}

thread_local! {
    /// Later trending pages already loaded, kept for Back.
    static LOADED: std::cell::RefCell<Option<Loaded>> = const { std::cell::RefCell::new(None) };
}

#[component]
pub fn Home() -> impl IntoView {
    let query = use_query_map();
    let sort = Memo::new(move |_| Sort::parse(query.read().get("sort").as_deref(), Sort::Hot));
    let data = LocalResource::new(|| async { api::get::<HomePayload>(&home_path()).await });
    // Keep page boundaries so appending a page does not move earlier cards.
    // Each page is ranked using the currently selected sort.
    let (kept, kept_offset) = LOADED
        .with(|l| l.borrow().clone())
        .filter(|loaded| js_sys::Date::now() - loaded.at < KEEP_MS)
        .map(|loaded| (loaded.pages, loaded.next_offset))
        .unwrap_or((Vec::new(), 100));
    let more = RwSignal::new(kept);
    let next_offset = RwSignal::new(kept_offset);
    let loading = RwSignal::new(false);
    Effect::new(move |_| set_title(if sort.get() == Sort::Hot { "" } else { sort.get().as_str() }));
    // A failed page is tried again a few times, a little later each time:
    // the end of the list stays where the reader is waiting for it.
    let failures = StoredValue::new(0u32);
    let retry = StoredValue::new(None::<Callback<()>>);
    let load_more = Callback::new(move |()| {
        let offset = next_offset.get_untracked();
        if loading.get_untracked() || offset > LAST_OFFSET {
            return;
        }
        loading.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let result = api::get::<HomePayload>(&home_page_path(offset)).await;
            // The request can finish after navigation disposed this page.
            if next_offset.is_disposed() {
                return;
            }
            if let Ok(page) = result {
                failures.set_value(0);
                let mut known: std::collections::HashSet<String> = more.with_untracked(|pages| {
                    pages.iter().flat_map(|page| &page.casts).map(|c| c.hash.clone()).collect()
                });
                let fresh: Vec<Cast> =
                    top_level(&page.data.pool).into_iter().filter(|c| known.insert(c.hash.clone())).collect();
                more.update(|pages| pages.push(LoadedPage { casts: fresh, notes: page.data.notes.clone() }));
                next_offset.set(offset + 100);
                LOADED.with(|l| {
                    *l.borrow_mut() = Some(Loaded {
                        at: js_sys::Date::now(),
                        pages: more.get_untracked(),
                        next_offset: offset + 100,
                    });
                });
            } else if matches!(result, Err(api::Failure::NotFound(_))) || failures.get_value() >= MAX_RETRIES {
                next_offset.set(LAST_OFFSET + 100);
            } else {
                failures.update_value(|f| *f += 1);
                let wait = std::time::Duration::from_secs(1 << failures.get_value());
                // Nothing to do if the reader has left the page meanwhile.
                set_timeout(
                    move || {
                        if let Some(Some(again)) = retry.try_get_value() {
                            again.run(());
                        }
                    },
                    wait,
                );
            }
            loading.set(false);
        });
    });
    retry.set_value(Some(load_more));

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
            Some(Ok(Fetched { data, stale, .. })) => {
                let payload: Arc<HomePayload> = data;
                let first_notes = payload.notes.clone();
                let notes = Memo::new(move |_| {
                    let mut notes = first_notes.clone();
                    more.with(|pages| {
                        for page in pages {
                            notes.merge(&page.notes);
                        }
                    });
                    Arc::new(notes)
                });
                let pool = top_level(&payload.pool);
                let first_hashes: std::collections::HashSet<String> = pool.iter().map(|c| c.hash.clone()).collect();
                // Re-sorting reorders the list in place instead of rebuilding the page.
                let casts = Signal::derive(move || {
                    let order = sort.get();
                    let mut all = sorted(&pool, order);
                    more.with(|pages| {
                        for page in pages {
                            all.extend(
                                sorted(&page.casts, order).into_iter().filter(|c| !first_hashes.contains(&c.hash)),
                            );
                        }
                    });
                    all
                });
                view! {
                    <Columns side=side(payload.channels.clone())>
                        <Heading sort/>
                        <StaleNotice stale/>
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

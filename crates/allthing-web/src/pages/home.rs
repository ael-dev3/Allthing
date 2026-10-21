// SPDX-License-Identifier: AGPL-3.0-or-later
//! The front page: trending casts, ranked locally.

use std::sync::Arc;

use allthing_core::api::{Annotations, HomePayload, IndexCoverage, LatestPayload, home_page_path, latest_path};
use allthing_core::format::full_number;
use allthing_core::listing::top_level;
use allthing_core::model::Cast;
use allthing_core::ranking::{Sort, TimeRange, TrendingWindow, sorted_for};
use allthing_core::time::format_date;
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::api::{self, Fetched};
use crate::components::cast::{CastList, CastSkeletons, after_paint};
use crate::components::clients::{ClientChip, ClientShareBar, client_param};
use crate::components::layout::{Columns, ErrorPanel, SortTabs, StaleNotice, TimeFilter};
use crate::util::{href, set_title, with_query};
use allthing_core::clients::from_client;

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
    window: TrendingWindow,
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
    let range = Memo::new(move |_| TimeRange::parse(query.read().get("t").as_deref()));
    Effect::new(move |_| set_title(if sort.get() == Sort::Hot { "" } else { sort.get().as_str() }));
    // Leaving the front page before its cards are drawn: its first cards,
    // drawn before the app came, can't stay over the next page.
    on_cleanup(crate::boot::hand_over);
    // The tab tapped shows as chosen in the next frame, and the feed follows
    // in a task of its own once that frame is painted: a tap is answered at
    // once, even where drawing the cards takes a while (docs/PERFORMANCE.md).
    let listed = after_a_paint(sort);
    // "new" lists the newest casts from the event stream; hot and top rank
    // the trending pool over the time range (and switching between them
    // re-sorts in place).
    let newest = Memo::new(move |_| listed.get() == Sort::New);
    move || {
        if newest.get() {
            view! { <Latest sort range/> }.into_any()
        } else {
            view! { <Trending sort listed range/> }.into_any()
        }
    }
}

/// `sort`, a paint later.
fn after_a_paint(sort: Memo<Sort>) -> ReadSignal<Sort> {
    let (later, set_later) = signal(sort.get_untracked());
    Effect::new(move |_| {
        if sort.get() != later.get_untracked() {
            after_paint(move || {
                if let Some(now) = sort.try_get_untracked() {
                    let _ = set_later.try_set(now);
                }
            });
        }
    });
    later
}

/// The newest casts on Farcaster, read from Snapchain's event stream and
/// rebuilt at the edge every 30 seconds.
#[component]
fn Latest(sort: Memo<Sort>, range: Memo<TimeRange>) -> impl IntoView {
    let data = LocalResource::new(|| async { api::get::<LatestPayload>(&latest_path()).await });
    // Only another client filter redraws the list: a tap on a sort tab
    // changes the query too, and mustn't rebuild the cards it's leaving.
    let query = use_query_map();
    let client = Memo::new(move |_| client_param(&query.read()));
    move || match data.get() {
        None => view! {
            <Columns>
                <Heading sort range/>
                <CastSkeletons/>
            </Columns>
        }
        .into_any(),
        Some(Err(failure)) => view! { <ErrorPanel failure/> }.into_any(),
        Some(Ok(Fetched { data, stale, .. })) => {
            let notes = Arc::new(data.notes.clone());
            let client = client.get();
            let casts = top_level(&data.pool);
            let casts = match client {
                Some(fid) => from_client(casts, &notes, fid),
                None => casts,
            };
            view! {
                <Columns>
                    <Heading sort range/>
                    {client.map(|fid| view! { <ClientChip fid notes=notes.clone()/> })}
                    <StaleNotice stale/>
                    <CastList casts=Signal::stored(casts) notes=Signal::stored(notes)/>
                </Columns>
            }
            .into_any()
        }
    }
}

/// Trending casts over the time range, ranked hot or top, with more pages
/// as the reader scrolls.
#[component]
fn Trending(sort: Memo<Sort>, listed: ReadSignal<Sort>, range: Memo<TimeRange>) -> impl IntoView {
    let leaning = crate::leanings::use_leaning();
    let window = Memo::new(move |_| range.get().window());
    let data = LocalResource::new(move || {
        let window = window.get();
        async move { api::get::<HomePayload>(&home_page_path(0, window)).await }
    });
    // Keep page boundaries so appending a page does not move earlier cards.
    // Each page is ranked using the currently selected sort.
    let (kept, kept_offset) = LOADED
        .with(|l| l.borrow().clone())
        .filter(|loaded| js_sys::Date::now() - loaded.at < KEEP_MS && loaded.window == window.get_untracked())
        .map(|loaded| (loaded.pages, loaded.next_offset))
        .unwrap_or((Vec::new(), 100));
    let more = RwSignal::new(kept);
    let next_offset = RwSignal::new(kept_offset);
    let loading = RwSignal::new(false);
    // A failed page is tried again a few times, a little later each time:
    // the end of the list stays where the reader is waiting for it.
    let failures = StoredValue::new(0u32);
    let retry = StoredValue::new(None::<Callback<()>>);
    let retry_timer = StoredValue::new(None::<TimeoutHandle>);
    let revision = StoredValue::new(0u64);
    let cancel_retry = move || {
        if let Some(Some(handle)) = retry_timer.try_get_value() {
            handle.clear();
        }
        retry_timer.set_value(None);
    };
    on_cleanup(move || {
        if let Some(Some(handle)) = retry_timer.try_get_value() {
            handle.clear();
        }
    });
    let load_more = Callback::new(move |()| {
        let offset = next_offset.get_untracked();
        if loading.get_untracked() || offset > LAST_OFFSET {
            return;
        }
        loading.set(true);
        let asked = window.get_untracked();
        let requested = revision.get_value();
        wasm_bindgen_futures::spawn_local(async move {
            let result = api::get::<HomePayload>(&home_page_path(offset, asked)).await;
            // The request can finish after navigation disposed this page, or
            // after the reader picked another time range.
            if revision.try_get_value() != Some(requested) || window.get_untracked() != asked {
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
                        window: asked,
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
                retry_timer.set_value(
                    set_timeout_with_handle(
                        move || {
                            if revision.try_get_value() == Some(requested)
                                && let Some(Some(again)) = retry.try_get_value()
                            {
                                retry_timer.set_value(None);
                                loading.set(false);
                                again.run(());
                            }
                        },
                        wait,
                    )
                    .ok(),
                );
                // Keep further observer callbacks from bypassing backoff.
                return;
            }
            loading.set(false);
        });
    });
    retry.set_value(Some(load_more));
    // Another time range is another list: start it from its first page.
    Effect::new(move |previous: Option<TrendingWindow>| {
        let current = window.get();
        if previous.is_some_and(|p| p != current) {
            revision.update_value(|r| *r = r.wrapping_add(1));
            cancel_retry();
            loading.set(false);
            more.set(Vec::new());
            next_offset.set(100);
            failures.set_value(0);
        }
        current
    });

    move || {
        match data.get() {
            None => view! {
                <Columns>
                    <Heading sort range/>
                    <CastSkeletons/>
                </Columns>
            }
            .into_any(),
            Some(Err(failure)) => {
                // Its first cards, drawn before the app came, can't stay.
                crate::boot::hand_over();
                view! { <ErrorPanel failure/> }.into_any()
            }
            Some(Ok(Fetched { data, stale, .. })) => {
                let payload: Arc<HomePayload> = data;
                // A range the index can't rank yet shows the past week.
                let shown = match window.get() {
                    asked if asked.is_indexed() && payload.index.is_none() => TrendingWindow::Week,
                    asked => asked,
                };
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
                let query = use_query_map();
                let client = Memo::new(move |_| client_param(&query.read()));
                // Re-sorting, or a favorite made, reorders the list in place
                // instead of rebuilding the page.
                let casts = Memo::new(move |_| {
                    let order = listed.get();
                    let leanings = leaning.leanings();
                    let mut all = sorted_for(&pool, order, &leanings);
                    more.with(|pages| {
                        for page in pages {
                            all.extend(
                                sorted_for(&page.casts, order, &leanings)
                                    .into_iter()
                                    .filter(|c| !first_hashes.contains(&c.hash)),
                            );
                        }
                    });
                    match client.get() {
                        Some(fid) => from_client(all, &notes.get(), fid),
                        None => all,
                    }
                });
                let chip_notes = Arc::new(payload.notes.clone());
                view! {
                    <Columns>
                        <Heading sort range/>
                        <IndexNote window=window.get() index=payload.index.clone()/>
                        <ClientShareBar casts=payload.pool.clone() notes=Arc::new(payload.notes.clone()) window=shown counted=payload.apps_24h.clone()/>
                        {move || client.get().map(|fid| view! { <ClientChip fid notes=chip_notes.clone()/> })}
                        <StaleNotice stale/>
                        {crate::components::outgoing_feed(None, crate::components::listed_hashes(casts.into()))}
                        <CastList casts notes on_end=load_more page_cursor=next_offset/>
                        <Show when=move || loading.get()><CastSkeletons count=2/></Show>
                    </Columns>
                }
                .into_any()
            }
        }
    }
}

#[component]
fn Heading(sort: Memo<Sort>, range: Memo<TimeRange>) -> impl IntoView {
    // "new" is newest first, whatever the range.
    let ranged = move || sort.get() != Sort::New;
    let query = use_query_map();
    view! {
        <div class="listing-head">
            <h1 class="visually-hidden">"Trending on Farcaster"</h1>
            {move || {
                let t = (range.get() != TimeRange::default()).then(|| range.get().as_str().to_owned());
                // The client filter stays when the reader changes sort or range.
                let client = client_param(&query.read()).map(|fid| fid.to_string());
                view! {
                    <div class="sort-row">
                        <SortTabs path=with_query(&href("/"), &[("t", t), ("client", client.clone())]) current=sort.get()/>
                        {ranged().then(|| view! { <TimeFilter path=with_query(&href("/"), &[("client", client)]) sort=sort.get() current=range.get()/> })}
                    </div>
                }
            }}
            {crate::components::layout::cast_pill()}
        </div>
    }
}

/// For Past month, Past year and All time: what Allthing's index ranked
/// them from, or that it can't rank them yet and the list is the past
/// week's trending casts.
#[component]
fn IndexNote(window: TrendingWindow, index: Option<IndexCoverage>) -> impl IntoView {
    window.is_indexed().then(|| {
        let text = match index {
            Some(index) => {
                let since = index.since.map(|ts| format!(" back to {}", format_date(ts))).unwrap_or_default();
                format!("Ranked from {} casts Allthing has indexed{since}.", full_number(index.casts))
            }
            None => {
                "Allthing's index can't rank this range yet, so this shows the past week's trending casts.".to_owned()
            }
        };
        view! { <p class="range-note">{text}</p> }
    })
}

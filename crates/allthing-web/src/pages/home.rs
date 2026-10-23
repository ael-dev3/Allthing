// SPDX-License-Identifier: AGPL-3.0-or-later
//! The front page: trending casts, ranked locally.

use std::sync::Arc;

use allthing_core::api::{Annotations, HomePayload, IndexCoverage, LatestPayload, home_page_path, latest_path};
use allthing_core::following::{FEED_PARAM, Feed, FollowingPayload, following_path};
use allthing_core::format::full_number;
use allthing_core::listing::top_level;
use allthing_core::model::Cast;
use allthing_core::ranking::{Sort, TimeRange, TrendingWindow, sorted_for};
use allthing_core::time::format_date;
use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_query_map};

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
    // Its title whenever it shows; kept while another page shows (kept.rs),
    // it leaves that page's alone.
    let location = use_location();
    Effect::new(move |_| {
        if crate::kept::is_front(&location.pathname.get()) {
            set_title(if sort.get() == Sort::Hot { "" } else { sort.get().as_str() });
        }
    });
    // Leaving the front page before its cards are drawn: its first cards,
    // drawn before the app came, can't stay over the next page.
    on_cleanup(crate::boot::hand_over);
    // Allthing's poll, beside the feed or in it, whichever list it shows.
    crate::components::provide_poll();
    // The tab tapped shows as chosen in the next frame, and the feed follows
    // in a task of its own once that frame is painted: a tap is answered at
    // once, even where drawing the cards takes a while (docs/PERFORMANCE.md).
    let listed = after_a_paint(sort);
    // For you's "new" lists the newest casts from the event stream; its hot
    // and top rank the trending pool over the time range (and switching
    // between them re-sorts in place). Following lists the casts of whom a
    // signed-in reader follows, in all three.
    let newest = Memo::new(move |_| listed.get() == Sort::New);
    let feed = feed_memo();
    move || {
        if feed.get() == Feed::Following {
            view! { <Following sort listed range/> }.into_any()
        } else if newest.get() {
            view! { <Latest sort range/> }.into_any()
        } else {
            view! { <Trending sort listed range/> }.into_any()
        }
    }
}

/// The front page's tab: Following when a signed-in reader asks for it
/// (`?feed=following`), For you otherwise (`allthing_core::following::Feed`).
fn feed_memo() -> Memo<Feed> {
    let reader = reader();
    let query = use_query_map();
    Memo::new(move |_| Feed::of(reader.get().is_some(), query.read().get(FEED_PARAM).as_deref()))
}

/// The signed-in reader's fid, as the session says; never in the static
/// copy, which has no sign-in.
#[cfg(not(feature = "static-host"))]
fn reader() -> Signal<Option<u64>> {
    let session = crate::components::signin::use_session();
    Signal::derive(move || session.user.with(|user| user.as_ref().map(|u| u.fid)))
}

#[cfg(feature = "static-host")]
fn reader() -> Signal<Option<u64>> {
    Signal::stored(None)
}

/// Whom For you lifts: the accounts a signed-in reader follows, as this
/// browser keeps them (`crate::followed`), renewed in the background once
/// the page is painted when they're old. None signed out.
fn followed_lift() -> StoredValue<std::collections::BTreeSet<u64>> {
    let Some(fid) = reader().get_untracked() else { return StoredValue::new(Default::default()) };
    after_paint(move || crate::followed::refresh(fid));
    StoredValue::new(crate::followed::followed(fid))
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
/// rebuilt at the edge every 30 seconds: newest first, with the casts of
/// accounts the reader sees less of lower (`sorted_for`).
#[component]
fn Latest(sort: Memo<Sort>, range: Memo<TimeRange>) -> impl IntoView {
    let leaning = crate::leanings::use_leaning();
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
            let pool = top_level(&data.pool);
            let pool = match client {
                Some(fid) => from_client(pool, &notes, fid),
                None => pool,
            };
            // Seeing less of an account, in a profile card over the list,
            // leaves its cards where they are until it's next loaded
            // (`Leaning::for_list`).
            let sorted_by = StoredValue::new(None);
            let casts = Memo::new(move |_| sorted_for(&pool, Sort::New, &leaning.for_list(sorted_by, ())));
            view! {
                <Columns class="front-columns">
                    <Heading sort range/>
                    // The apps the newest casts came from, as hot and top
                    // show their trending casts': the list starts where
                    // theirs does, so switching between them doesn't move it.
                    <ClientShareBar casts=crate::miniapps::as_shown(&data.pool) notes=notes.clone()/>
                    {client.map(|fid| view! { <ClientChip fid notes=notes.clone()/> })}
                    <StaleNotice stale/>
                    {crate::components::poll_gutter()}
                    <CastList casts notes=Signal::stored(notes) inset=crate::components::poll_inset()/>
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
    let followed = followed_lift();
    let window = Memo::new(move |_| range.get().window());
    let data = LocalResource::new(move || {
        let window = window.get();
        async move { api::get::<HomePayload>(&home_page_path(0, window)).await }
    });
    // The poll's counts, newer than the reader's own vote's, perhaps.
    Effect::new(move |_| {
        if let Some(Ok(fetched)) = data.get() {
            crate::components::poll_counts(fetched.data.poll, fetched.data.meta.fetched_at as f64);
        }
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
    // Whether the window's list has run out: a page came back empty.
    let ended = RwSignal::new(false);
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
                if page.data.pool.is_empty() {
                    ended.set(true);
                }
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
            ended.set(false);
            failures.set_value(0);
        }
        current
    });
    // A list whose first page is empty has nothing more to load.
    Effect::new(move |_| {
        if let Some(Ok(fetched)) = data.get()
            && fetched.data.pool.is_empty()
        {
            ended.set(true);
        }
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
                // Re-sorting reorders the list in place instead of rebuilding
                // the page. A favorite made, or Show less, in a profile card
                // over the list waits for the next sort, filter or load
                // (`Leaning::for_list`): its cards stay under the pointer.
                let sorted_by = StoredValue::new(None);
                let casts = Memo::new(move |_| -> Vec<Cast> {
                    let order = listed.get();
                    let filter = client.get();
                    let mut leanings = leaning.for_list(sorted_by, (order, filter));
                    // For you: the casts of whom the reader follows lifted.
                    leanings.followed = followed.get_value();
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
                    let all = match filter {
                        Some(fid) => from_client(all, &notes.get(), fid),
                        None => all,
                    };
                    // A copy from the edge, or the app's own, can be older than
                    // its window: what has aged out since is left out here too.
                    all.into_iter().filter(|cast| shown.holds(cast.ts, now_secs())).collect()
                });
                let chip_notes = Arc::new(payload.notes.clone());
                let in_window: Vec<Cast> =
                    payload.pool.iter().filter(|cast| shown.holds(cast.ts, now_secs())).cloned().collect();
                view! {
                    <Columns class="front-columns">
                        <Heading sort range/>
                        <IndexNote window=window.get() index=payload.index.clone()/>
                        <ClientShareBar casts=crate::miniapps::as_shown(&in_window) notes=Arc::new(payload.notes.clone()) window=shown counted=payload.apps_24h.clone()/>
                        {move || client.get().map(|fid| view! { <ClientChip fid notes=chip_notes.clone()/> })}
                        <StaleNotice stale/>
                        {crate::components::outgoing_feed(None, crate::components::listed_hashes(casts.into()))}
                        {crate::components::poll_gutter()}
                        <CastList casts notes on_end=load_more page_cursor=next_offset inset=crate::components::poll_inset()/>
                        <Show when=move || loading.get()><CastSkeletons count=2/></Show>
                        // Under the list, where it ends, so the cards above don't move.
                        {move || (ended.get() && !window.get().is_indexed()).then(|| {
                            view! { <p class="range-note">{window_note(shown, casts.get().len())}</p> }
                        })}
                    </Columns>
                }
                .into_any()
            }
        }
    }
}

#[derive(Clone)]
struct FollowingPage {
    casts: Vec<Cast>,
    notes: Annotations,
}

/// The Following tab: only the casts of the accounts the reader follows
/// (`/api/v1/following`), ranked as the trending pool is in hot and top,
/// with their favorites and those they see less of, newest first in new,
/// and over the time range; older pages as they scroll. A session that has
/// ended shows everyone's.
#[component]
fn Following(sort: Memo<Sort>, listed: ReadSignal<Sort>, range: Memo<TimeRange>) -> impl IntoView {
    let leaning = crate::leanings::use_leaning();
    let data = LocalResource::new(|| async { api::get::<FollowingPayload>(&following_path(None)).await });
    let more = RwSignal::new(Vec::<FollowingPage>::new());
    let next = RwSignal::new(None::<String>);
    let pages = RwSignal::new(0u32);
    let loading = RwSignal::new(false);
    let alive = StoredValue::new(());
    let load_more = Callback::new(move |()| {
        let Some(cursor) = next.get_untracked() else { return };
        if loading.get_untracked() {
            return;
        }
        loading.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let result = api::get::<FollowingPayload>(&following_path(Some(&cursor))).await;
            if alive.try_get_value().is_none() {
                return;
            }
            match result {
                Ok(page) => {
                    let mut known: std::collections::HashSet<String> = more
                        .with_untracked(|pages| pages.iter().flat_map(|p| &p.casts).map(|c| c.hash.clone()).collect());
                    let fresh = page.data.pool.iter().filter(|c| known.insert(c.hash.clone())).cloned().collect();
                    more.update(|pages| pages.push(FollowingPage { casts: fresh, notes: page.data.notes.clone() }));
                    next.set(page.data.next.clone());
                }
                // The end of the list, rather than asking again and again.
                Err(_) => next.set(None),
            }
            pages.update(|n| *n += 1);
            loading.set(false);
        });
    });
    move || match data.get() {
        None => view! {
            <Columns>
                <Heading sort range/>
                <CastSkeletons/>
            </Columns>
        }
        .into_any(),
        // Signed out meanwhile (the session ran out), or a Worker that
        // doesn't have it yet (while a deploy rolls out): everyone's feed.
        Some(Err(api::Failure::Refused(_) | api::Failure::NotFound(_))) => {
            view! { <Trending sort listed range/> }.into_any()
        }
        Some(Err(failure)) => {
            crate::boot::hand_over();
            view! { <ErrorPanel failure/> }.into_any()
        }
        Some(Ok(Fetched { data, stale, .. })) => {
            let payload: Arc<FollowingPayload> = data;
            if let Some(fid) = reader().get_untracked() {
                crate::followed::remember(fid, &payload);
            }
            next.set(payload.next.clone());
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
            let pool = payload.pool.clone();
            let first_hashes: std::collections::HashSet<String> = pool.iter().map(|c| c.hash.clone()).collect();
            let query = use_query_map();
            let client = Memo::new(move |_| client_param(&query.read()));
            let sorted_by = StoredValue::new(None);
            let casts = Memo::new(move |_| {
                let order = listed.get();
                let filter = client.get();
                let leanings = leaning.for_list(sorted_by, (order, filter));
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
                let window = range.get().window();
                let all = all.into_iter().filter(|cast| window.holds(cast.ts, now_secs())).collect();
                match filter {
                    Some(fid) => from_client(all, &notes.get(), fid),
                    None => all,
                }
            });
            let note = following_note(payload.pool.len());
            let chip_notes = Arc::new(payload.notes.clone());
            let shared: Vec<Cast> = payload.pool.clone();
            view! {
                <Columns class="front-columns">
                    <Heading sort range/>
                    <p class="range-note following-note">{note}</p>
                    <ClientShareBar casts=shared notes=Arc::new(payload.notes.clone())/>
                    {move || client.get().map(|fid| view! { <ClientChip fid notes=chip_notes.clone()/> })}
                    <StaleNotice stale/>
                    {crate::components::outgoing_feed(None, crate::components::listed_hashes(casts.into()))}
                    {crate::components::poll_gutter()}
                    <CastList casts notes on_end=load_more page_cursor=pages inset=crate::components::poll_inset()/>
                    <Show when=move || loading.get()><CastSkeletons count=2/></Show>
                </Columns>
            }
            .into_any()
        }
    }
}

/// What the note over the Following tab says: whose casts it lists.
fn following_note(theirs: usize) -> &'static str {
    match theirs {
        0 => "No one you follow has cast lately.",
        _ => "Casts from people you follow.",
    }
}

#[component]
fn Heading(sort: Memo<Sort>, range: Memo<TimeRange>) -> impl IntoView {
    // "new" is newest first, whatever the range.
    let ranged = move || sort.get() != Sort::New;
    let query = use_query_map();
    let signed_in = Signal::derive({
        let reader = reader();
        move || reader.get().is_some()
    });
    let feed = feed_memo();
    view! {
        <div class="listing-head">
            <h1 class="visually-hidden">"Trending on Farcaster"</h1>
            {move || {
                let t = (range.get() != TimeRange::default()).then(|| range.get().as_str().to_owned());
                // The client filter stays when the reader changes sort or range.
                let client = client_param(&query.read()).map(|fid| fid.to_string());
                // So does the tab.
                let tab = feed.get().param();
                let sort_kept = (sort.get() != Sort::Hot).then(|| sort.get().as_str().to_owned());
                view! {
                    {signed_in.get().then(|| view! { <FeedTabs current=feed.get() sort=sort_kept t=t.clone() client=client.clone()/> })}
                    <div class="sort-row">
                        <SortTabs path=with_query(&href("/"), &[("t", t), ("client", client.clone()), (FEED_PARAM, tab.clone())]) current=sort.get()/>
                        {ranged().then(|| {
                            view! { <TimeFilter path=with_query(&href("/"), &[("client", client), (FEED_PARAM, tab)]) sort=sort.get() current=range.get()/> }
                        })}
                    </div>
                }
            }}
        </div>
    }
}

/// A signed-in reader's two feeds, as X's: **For you** (the trending pool,
/// with the casts of whom they follow lifted) and **Following** (only
/// theirs, `?feed=following`). Links, keeping the sort, the time range and
/// the client filter; signed out there is only For you, and no tabs.
#[component]
fn FeedTabs(current: Feed, sort: Option<String>, t: Option<String>, client: Option<String>) -> impl IntoView {
    view! {
        <nav class="feed-tabs" aria-label="Feed">
            {[Feed::ForYou, Feed::Following]
                .into_iter()
                .map(|feed| {
                    let to = with_query(
                        &href("/"),
                        &[("sort", sort.clone()), ("t", t.clone()), ("client", client.clone()), (FEED_PARAM, feed.param())],
                    );
                    view! {
                        <a href=to data-noscroll="" class:selected=feed == current aria-current=(feed == current).then_some("page")>
                            {feed.label()}
                        </a>
                    }
                })
                .collect_view()}
        </nav>
    }
}

/// The reader's clock, in seconds since the Unix epoch, as the casts' times count.
fn now_secs() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

/// What a trending window's list holds once it has run out: the casts it
/// has, counted, rather than a list padded with older ones.
fn window_note(window: TrendingWindow, count: usize) -> String {
    let past = window.past();
    match count {
        0 => format!("No casts in {past}."),
        1 => format!("Only 1 cast in {past}."),
        n => format!("Only {} casts in {past}.", full_number(n as u64)),
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

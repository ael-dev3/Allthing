// SPDX-License-Identifier: AGPL-3.0-or-later
//! Saved posts, public reactions and private reading history, with bounded pages.

use leptos::prelude::*;

pub fn page() -> AnyView {
    #[cfg(not(feature = "static-host"))]
    {
        crate::split::page(with_worker::library_page()).into_any()
    }
    #[cfg(feature = "static-host")]
    {
        crate::util::set_title("Library");
        view! {
            <section class="library-page">
                <h1 class="page-title">"Library"</h1>
                <div class="library-empty">
                    <p>"Your Library needs a sign-in on Allthing's server."</p>
                    <a class="button" href="https://allthing.social/library">"Open your Library on allthing.social"</a>
                </div>
            </section>
        }
        .into_any()
    }
}

/// Called by the account controller, including sign-out and A → B → A.
/// In-flight page work also carries this epoch, so it cannot refill the cache.
#[cfg(not(feature = "static-host"))]
pub fn clear_cached() {
    with_worker::clear_cached();
}

#[cfg(not(feature = "static-host"))]
pub mod with_worker {
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::Arc;

    use allthing_core::api::{LibraryCastsPayload, LibraryPayload, ReactionKind, library_casts_path, library_path};
    use allthing_core::model::Cast;
    use leptos::prelude::*;
    use leptos::task::spawn_local;
    use leptos_router::NavigateOptions;

    use crate::Notes;
    use crate::api;
    use crate::components::bookmark::BookmarkButton;
    use crate::components::cast::{CastList, CastSkeletons};
    use crate::components::chips::TimeAgo;
    use crate::components::signin::use_session;
    use crate::library::{Library, PrivateCursor, PrivateKind, SyncStatus, use_library};
    use crate::util::{cast_href, element_by_id, library_href, set_title, use_go, use_query_param, with_query};

    const PANEL: &str = "library-panel";
    const CACHE_MS: f64 = 300_000.0;
    // Retain ordinary deep Back visits without keeping an entire account archive.
    const CACHE_ROWS: usize = 1_000;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    enum Tab {
        Bookmarks,
        Likes,
        Recasts,
        Seen,
    }

    impl Tab {
        const ALL: [Self; 4] = [Self::Bookmarks, Self::Likes, Self::Recasts, Self::Seen];
        fn parse(value: Option<&str>) -> Self {
            match value {
                Some("likes") => Self::Likes,
                Some("recasts") => Self::Recasts,
                Some("seen") => Self::Seen,
                _ => Self::Bookmarks,
            }
        }
        fn query(self) -> &'static str {
            match self {
                Self::Bookmarks => "bookmarks",
                Self::Likes => "likes",
                Self::Recasts => "recasts",
                Self::Seen => "seen",
            }
        }
        fn label(self) -> &'static str {
            match self {
                Self::Bookmarks => "Bookmarks",
                Self::Likes => "Likes",
                Self::Recasts => "Recasts",
                Self::Seen => "Seen",
            }
        }
        fn private(self) -> Option<PrivateKind> {
            match self {
                Self::Bookmarks => Some(PrivateKind::Bookmarks),
                Self::Seen => Some(PrivateKind::Seen),
                _ => None,
            }
        }
        fn reaction(self) -> Option<ReactionKind> {
            match self {
                Self::Likes => Some(ReactionKind::Likes),
                Self::Recasts => Some(ReactionKind::Recasts),
                _ => None,
            }
        }
        fn empty(self) -> &'static str {
            match self {
                Self::Bookmarks => "No bookmarks yet. Save a post to find it here.",
                Self::Likes => "No likes were returned by Farcaster.",
                Self::Recasts => "No recasts were returned by Farcaster.",
                Self::Seen => "Your reading history will appear here as you read posts.",
            }
        }
        fn activity(self) -> &'static str {
            match self {
                Self::Bookmarks => "Saved",
                Self::Likes => "Liked",
                Self::Recasts => "Recast",
                Self::Seen => "Seen",
            }
        }
    }

    fn tab_href(tab: Tab, query: &str) -> String {
        with_query(
            &library_href(),
            &[
                ("tab", (tab != Tab::Bookmarks).then(|| tab.query().to_owned())),
                ("q", (!query.trim().is_empty()).then(|| query.trim().chars().take(200).collect())),
            ],
        )
    }

    #[derive(Clone)]
    enum Cursor {
        Private(PrivateCursor),
        Public(String),
    }

    #[derive(Clone, Default)]
    struct Listing {
        pool: Vec<Cast>,
        times: BTreeMap<String, i64>,
        unavailable: Vec<String>,
        filtered: Vec<String>,
        notes: Notes,
        next: Option<Cursor>,
        loaded: bool,
        // A cached private page keeps the revision at which it was read. Back
        // must offer a refresh if remote activity changed while it was away.
        private_revision: Option<u64>,
        stale: bool,
        at: f64,
    }

    thread_local! {
        static CACHE: RefCell<HashMap<(u64, Tab), Listing>> = RefCell::new(HashMap::new());
        static EPOCH: Cell<u64> = const { Cell::new(0) };
    }

    pub fn clear_cached() {
        EPOCH.with(|epoch| epoch.set(epoch.get().wrapping_add(1)));
        CACHE.with(|cache| cache.borrow_mut().clear());
    }

    fn remember(fid: u64, tab: Tab, listing: &Listing) {
        CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache.retain(|(owner, _), value| *owner == fid && js_sys::Date::now() - value.at < CACHE_MS);
            if listing.times.len() <= CACHE_ROWS {
                cache.insert((fid, tab), listing.clone());
            } else {
                cache.remove(&(fid, tab));
            }
        });
    }

    fn append(current: &mut Listing, page: Listing, reset: bool) {
        if reset {
            *current = Listing::default();
        }
        let mut hashes: HashSet<String> = current.pool.iter().map(|cast| cast.hash.clone()).collect();
        current.pool.extend(page.pool.into_iter().filter(|cast| hashes.insert(cast.hash.clone())));
        current.times.extend(page.times);
        for (into, extra) in [(&mut current.unavailable, page.unavailable), (&mut current.filtered, page.filtered)] {
            let mut known: HashSet<String> = into.iter().cloned().collect();
            into.extend(extra.into_iter().filter(|hash| known.insert(hash.clone())));
        }
        Arc::make_mut(&mut current.notes).merge(&page.notes);
        current.next = page.next;
        current.loaded = true;
        current.stale |= page.stale;
        current.at = js_sys::Date::now();
    }

    async fn fetch_page(library: Library, fid: u64, tab: Tab, cursor: Option<Cursor>) -> Result<Listing, String> {
        if let Some(kind) = tab.private() {
            let cursor = match cursor {
                None => None,
                Some(Cursor::Private(value)) => Some(value),
                _ => return Err("Invalid private Library cursor.".into()),
            };
            let page = library.page(kind, cursor).await?;
            let hashes: Vec<String> = page.entries.iter().map(|entry| entry.hash.clone()).collect();
            let times = page.entries.into_iter().map(|entry| (entry.hash, entry.at)).collect();
            if hashes.is_empty() {
                return Ok(Listing { times, next: page.next.map(Cursor::Private), ..Default::default() });
            }
            let fetched =
                api::get::<LibraryCastsPayload>(&library_casts_path(&hashes)).await.map_err(|error| error.message())?;
            let data = &fetched.data;
            Ok(Listing {
                pool: data.pool.clone(),
                times,
                unavailable: data.unavailable.clone(),
                filtered: data.filtered.clone(),
                notes: data.notes.clone().into(),
                next: page.next.map(Cursor::Private),
                stale: fetched.stale,
                ..Default::default()
            })
        } else {
            let kind = tab.reaction().ok_or("Unknown Library tab.")?;
            let cursor = match cursor {
                None => None,
                Some(Cursor::Public(value)) => Some(value),
                _ => return Err("Invalid reaction cursor.".into()),
            };
            let fetched = api::get::<LibraryPayload>(&library_path(fid, kind, cursor.as_deref()))
                .await
                .map_err(|error| error.message())?;
            let data = &fetched.data;
            if data.fid != fid || data.kind != kind {
                return Err("The Library answer belongs to another account or tab.".into());
            }
            Ok(Listing {
                pool: data.pool.clone(),
                times: data.entries.iter().map(|entry| (entry.hash.clone(), entry.at)).collect(),
                unavailable: data.unavailable.clone(),
                filtered: data.filtered.clone(),
                notes: data.notes.clone().into(),
                next: data.next.clone().map(Cursor::Public),
                stale: fetched.stale,
                ..Default::default()
            })
        }
    }

    #[leptos::wasm_split::wasm_split(library, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_library))]
    pub fn library_page() -> Result<AnyView, leptos::wasm_split::SplitLoaderError> {
        Ok(view! { <LibraryPage/> }.into_any())
    }

    #[component]
    fn LibraryPage() -> impl IntoView {
        set_title("Library");
        let session = use_session();
        let tab = use_query_param("tab");
        let query = use_query_param("q");
        let selected = Memo::new(move |_| tab.with(|tab| Tab::parse(tab.as_deref())));
        let whose = Memo::new(move |_| session.user.with(|user| user.as_ref().map(|user| user.fid)));
        view! {
            <section class="library-page">
                <div class="library-heading"><h1 class="page-title">"Library"</h1></div>
                <p class="library-privacy">"Bookmarks and seen history are private to your account. Likes and recasts are public on Farcaster."</p>
                <Tabs selected query/>
                <Filter selected query/>
                <div id=PANEL class="library-panel" role="tabpanel" aria-labelledby={move || format!("library-tab-{}", selected.get().query())}>
                    <For
                        each={move || whose.get().into_iter().map(|fid| (fid, selected.get())).collect::<Vec<_>>()}
                        key=|(fid, tab)| (*fid, *tab)
                        children={move |(fid, tab)| view! { <List fid=fid tab=tab query=query/> }}
                    />
                    <Show when=move || whose.with(Option::is_none)>
                        <div class="library-empty">
                            <p>"Sign in to see your Library."</p>
                            <button type="button" class="button" on:click=move |_| session.sign_in()>"Sign in"</button>
                        </div>
                    </Show>
                </div>
            </section>
        }
    }

    #[component]
    fn Tabs(selected: Memo<Tab>, query: Memo<Option<String>>) -> impl IntoView {
        let go = use_go();
        let keys = move |event: leptos::ev::KeyboardEvent| {
            let at = Tab::ALL.iter().position(|tab| *tab == selected.get_untracked()).unwrap_or(0);
            let next = match event.key().as_str() {
                "ArrowRight" => (at + 1) % 4,
                "ArrowLeft" => (at + 3) % 4,
                "Home" => 0,
                "End" => 3,
                _ => return,
            };
            event.prevent_default();
            let to = Tab::ALL[next];
            go(
                &tab_href(to, &query.get_untracked().unwrap_or_default()),
                NavigateOptions { scroll: false, ..Default::default() },
            );
            request_animation_frame(move || {
                if let Some(element) = element_by_id::<web_sys::HtmlElement>(&format!("library-tab-{}", to.query())) {
                    let _ = element.focus();
                }
            });
        };
        view! {
            <div class="tabs library-tabs" role="tablist" aria-label="Library" on:keydown=keys>
                {Tab::ALL.into_iter().map(|tab| view! {
                    <a role="tab" id=format!("library-tab-{}", tab.query()) href=move || tab_href(tab, &query.get().unwrap_or_default())
                        aria-controls=PANEL aria-selected=move || (selected.get() == tab).to_string()
                        tabindex=move || if selected.get() == tab { "0" } else { "-1" }
                        data-noscroll="" class:selected=move || selected.get() == tab>{tab.label()}</a>
                }).collect_view()}
            </div>
        }
    }

    #[component]
    fn Filter(selected: Memo<Tab>, query: Memo<Option<String>>) -> impl IntoView {
        let input = NodeRef::<leptos::html::Input>::new();
        let go = use_go();
        view! {
            <form class="library-filter" on:submit=move |event| {
                event.prevent_default();
                let typed = input.get_untracked().map(|input| input.value()).unwrap_or_default();
                go(&tab_href(selected.get_untracked(), &typed), NavigateOptions { scroll: false, ..Default::default() });
            }>
                <label for="library-filter">"Filter loaded posts"</label>
                <div class="library-filter-row">
                    <input id="library-filter" type="search" maxlength="200" placeholder="Text or author in loaded posts" node_ref=input prop:value=move || query.get().unwrap_or_default()/>
                    <button type="submit" class="button">"Filter"</button>
                    <Show when=move || query.with(|query| query.as_ref().is_some_and(|query| !query.trim().is_empty()))>
                        <a class="account-link" href=move || tab_href(selected.get(), "") data-noscroll="">"Clear"</a>
                    </Show>
                </div>
                <p class="muted small">"This filters the pages you've loaded. Load more to include older posts."</p>
            </form>
        }
    }

    #[component]
    fn List(fid: u64, tab: Tab, query: Memo<Option<String>>) -> impl IntoView {
        let session = use_session();
        let library = use_library();
        let cached = CACHE
            .with(|cache| {
                cache.borrow().get(&(fid, tab)).filter(|value| js_sys::Date::now() - value.at < CACHE_MS).cloned()
            })
            .unwrap_or_default();
        let listing = RwSignal::new(cached);
        let loading = RwSignal::new(false);
        let failure = RwSignal::new(None::<String>);
        let message = RwSignal::new(None::<String>);
        let changed = RwSignal::new(false);
        let request = StoredValue::new(0_u64);
        let retry_reset = StoredValue::new(true);
        let loads = RwSignal::new(0_u32);
        let last_revision = StoredValue::new(
            listing.with_untracked(|value| value.private_revision).unwrap_or_else(|| untrack(|| library.revision())),
        );
        let load = Callback::new(move |reset: bool| {
            if loading.get_untracked()
                || session.user.with_untracked(|user| user.as_ref().map(|user| user.fid)) != Some(fid)
            {
                return;
            }
            if tab.private().is_some() && !matches!(library.status(), SyncStatus::Ready) {
                library.retry();
                return;
            }
            let cursor = if reset { None } else { listing.with_untracked(|value| value.next.clone()) };
            request.update_value(|value| *value = value.wrapping_add(1));
            let id = request.get_value();
            retry_reset.set_value(reset);
            let epoch = EPOCH.with(Cell::get);
            loading.set(true);
            failure.set(None);
            spawn_local(async move {
                let result = fetch_page(library, fid, tab, cursor).await;
                if request.try_get_value() != Some(id)
                    || EPOCH.with(Cell::get) != epoch
                    || session.user.with_untracked(|user| user.as_ref().map(|user| user.fid)) != Some(fid)
                {
                    return;
                }
                match result {
                    Ok(page) => {
                        listing.update(|current| {
                            append(current, page, reset);
                            current.private_revision = tab.private().map(|_| untrack(|| library.revision()));
                        });
                        listing.with_untracked(|current| remember(fid, tab, current));
                        last_revision.set_value(untrack(|| library.revision()));
                        changed.set(false);
                    }
                    Err(error) => failure.set(Some(error)),
                }
                loading.set(false);
                loads.update(|value| *value = value.wrapping_add(1));
            });
        });
        Effect::new(move |_| {
            let ready = tab.private().is_none() || matches!(library.status(), SyncStatus::Ready);
            if ready
                && !listing.with_untracked(|value| value.loaded)
                && !loading.get_untracked()
                && failure.with_untracked(Option::is_none)
            {
                load.run(true);
            }
        });
        Effect::new(move |_| {
            let revision = library.revision();
            if tab.private().is_some()
                && revision != last_revision.get_value()
                && listing.with_untracked(|value| value.loaded)
            {
                changed.set(true);
            }
        });
        let filtered: Signal<Vec<Cast>> = Memo::new(move |_| {
            let text = query.get().unwrap_or_default().trim().to_lowercase();
            listing.with(|value| value.pool.iter().filter(|cast| matches_query(cast, &text)).cloned().collect())
        })
        .into();
        let notes = Signal::derive(move || listing.with(|value| value.notes.clone()));
        let extra = Callback::new(move |cast: Cast| {
            let hash = cast.hash.clone();
            let at = Signal::derive(move || listing.with(|value| value.times.get(&hash).copied()).unwrap_or(0));
            view! { <Activity hash=cast.hash at tab/> }.into_any()
        });
        view! {
            <div class="library-sync" role="status" aria-live="polite">
                {move || match library.status() {
                    SyncStatus::SignedOut => "Sign in to sync private Library data.".to_owned(),
                    SyncStatus::Connecting => "Connecting your private Library…".to_owned(),
                    SyncStatus::Ready => "Private Library connected.".to_owned(),
                    SyncStatus::Offline => "Offline. Private Library sync will resume when you're online.".to_owned(),
                    SyncStatus::Failed(_) => "Private Library could not connect.".to_owned(),
                }}
                <Show when=move || matches!(library.status(), SyncStatus::Offline | SyncStatus::Failed(_))>
                    <button type="button" class="account-link" on:click=move |_| library.retry()>"Retry sync"</button>
                </Show>
            </div>
            <div class="library-controls">
                <p class="muted small">{format!("Most recently {} first.", tab.activity().to_ascii_lowercase())}</p>
                <button type="button" class="account-link" disabled=loading on:click=move |_| {
                    if let Some(kind) = tab.reaction() { api::forget(&library_path(fid, kind, None)); }
                    load.run(true);
                }>"Refresh"</button>
            </div>
            <Show when=move || changed.get()>
                <div class="library-change" role="status">
                    "Your Library changed on this or another device. "
                    <button type="button" class="account-link" disabled=loading on:click=move |_| load.run(true)>"Refresh this list"</button>
                </div>
            </Show>
            <Show when=move || !listing.with(|value| value.loaded) && failure.with(Option::is_none)
                && (tab.private().is_none() || matches!(library.status(), SyncStatus::Ready | SyncStatus::Connecting))>
                <CastSkeletons count=3/>
            </Show>
            <Show when=move || !listing.with(|value| value.loaded) && tab.private().is_some()
                && matches!(library.status(), SyncStatus::Offline | SyncStatus::Failed(_))>
                <p class="library-empty">"Your private Library is unavailable until sync reconnects. No empty history is assumed."</p>
            </Show>
            <Show when=move || listing.with(|value| value.loaded)>
                <Show when=move || listing.with(|value| value.stale)>
                    <p class="library-note">"Showing the last available Farcaster data. Refresh to try for a newer copy."</p>
                </Show>
                <Show when=move || filtered.with(Vec::is_empty) && listing.with(|value| value.unavailable.is_empty() && value.filtered.is_empty())>
                    <p class="library-empty">{move || if query.with(|query| query.as_ref().is_some_and(|query| !query.trim().is_empty())) {
                        "No loaded posts match this filter. Clear it or load older pages."
                    } else { tab.empty() }}</p>
                </Show>
                <CastList casts=filtered notes page_cursor=loads row_footer=Some(extra)/>
                <Show when=move || listing.with(|value| value.pool.iter().any(|cast| cast.mini_app)) && !crate::miniapps::shown()>
                    <p class="library-note">"Some loaded posts have mini apps and are hidden by your Settings."</p>
                </Show>
                <Show when=move || listing.with(|value| !value.filtered.is_empty())>
                    <p class="library-note">{move || format!("{} loaded posts are hidden by content filters.", listing.with(|value| value.filtered.len()))}</p>
                </Show>
                <For each=move || listing.with(|value| value.unavailable.clone()) key=|hash| hash.clone() children=move |hash| {
                    let key = hash.clone();
                    let activity_hash = hash.clone();
                    let at = Signal::derive(move || listing.with(|value| value.times.get(&key).copied()).unwrap_or(0));
                    view! {
                        <article class="library-unavailable" data-library-hash=hash.clone()>
                            <p>"This post was not returned by Farcaster. It may be unavailable."</p>
                            <a href=cast_href(&hash)>"Try opening the post"</a>
                            <BookmarkButton hash=hash.clone()/>
                            <Activity hash=activity_hash at tab/>
                        </article>
                    }
                }/>
            </Show>
            <Show when=move || failure.with(Option::is_some)>
                <div class="library-error" role="alert">
                    <p>{move || failure.get()}</p>
                    <button type="button" class="button" on:click=move |_| load.run(retry_reset.get_value())>"Try again"</button>
                </div>
            </Show>
            <Show when=move || listing.with(|value| value.next.is_some())>
                <div class="library-more">
                    <button type="button" class="button" disabled=loading aria-busy=move || loading.get().then_some("true") on:click=move |_| load.run(false)>
                        {move || if loading.get() { "Loading…" } else { "Load more" }}
                    </button>
                </div>
            </Show>
            <p class="library-result" role="status">{move || message.get()}</p>
            {(tab == Tab::Seen).then(|| view! { <ClearHistory message/> })}
        }
    }

    fn matches_query(cast: &Cast, query: &str) -> bool {
        query.is_empty()
            || cast.text.to_lowercase().contains(query)
            || cast.author.username.to_lowercase().contains(query)
            || cast.author.display_name.to_lowercase().contains(query)
    }

    #[component]
    fn Activity(hash: String, at: Signal<i64>, tab: Tab) -> impl IntoView {
        let library = use_library();
        let message = RwSignal::new(None::<String>);
        let key = hash.clone();
        let marked =
            Memo::new(move |_| library.seen(&key) == Some(false) && !matches!(library.status(), SyncStatus::Failed(_)));
        view! {
            <div class="library-activity">
                <span>{tab.activity()}" "{move || (at.get() > 0).then(|| view! { <TimeAgo ts=at.get()/> })}</span>
                {(tab == Tab::Seen).then(|| view! {
                    <button type="button" class="account-link" disabled=marked on:click=move |_| {
                        match library.forget_seen(&hash) {
                            Ok(()) => message.set(Some("Mark-unread requested. Refresh this list after it syncs.".into())),
                            Err(error) => message.set(Some(error)),
                        }
                    }>{move || if marked.get() { "Unread requested" } else { "Mark unread" }}</button>
                })}
                <span class="library-action-result" role="status">{move || message.get()}</span>
            </div>
        }
    }

    #[component]
    fn ClearHistory(message: RwSignal<Option<String>>) -> impl IntoView {
        let library = use_library();
        let armed = RwSignal::new(false);
        view! {
            <div class="library-history-controls">
                <button type="button" class="account-link" aria-expanded=move || armed.get().to_string() on:click=move |_| armed.set(!armed.get_untracked())>"Clear reading history…"</button>
                <Show when=move || armed.get()>
                    <div class="library-confirm" role="group" aria-label="Confirm clearing reading history">
                        <p>"Clear reading history on all devices? Previously seen posts can appear in Home again. Your bookmarks stay saved."</p>
                        <button type="button" class="button" on:click=move |_| {
                            match library.clear_seen() {
                                Ok(()) => { message.set(Some("Reading-history reset requested.".into())); armed.set(false); }
                                Err(error) => message.set(Some(error)),
                            }
                        }>"Clear history on all devices"</button>
                        <button type="button" class="account-link" on:click=move |_| armed.set(false)>"Cancel"</button>
                    </div>
                </Show>
            </div>
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn loaded_filter_covers_unicode_text_and_authors_without_changing_order() {
            let cast: Cast = serde_json::from_str(r#"{"hash":"0x0000000000000000000000000000000000000001","text":"Café in Белград","ts":1,"author":{"fid":1,"username":"reader","display_name":"Alice Example"}}"#).unwrap();
            assert!(matches_query(&cast, "белград"));
            assert!(matches_query(&cast, "alice"));
            assert!(matches_query(&cast, "reader"));
            assert!(!matches_query(&cast, "not loaded"));
        }
    }
}

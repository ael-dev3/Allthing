// SPDX-License-Identifier: AGPL-3.0-or-later
//! The signed-in reader's notifications (`allthing_core::notifications`):
//! who liked, recasted, replied to, mentioned and quoted their casts, and
//! who followed them, grouped as the Farcaster app groups them, under the
//! tabs All, Mentions and Replies (`?tab=`). One request,
//! `/api/v1/notifications`, which the header's bell has usually made
//! already, and one more for each "Load more". Opening the page marks
//! everything on it as seen, so the bell clears; what was new stays marked
//! for this visit. Not in the static copy, which has no sign-in.

use leptos::prelude::*;

/// The page, as its route shows it. In front of the Worker, code of its
/// own (`crate::split`), fetched as a link to it is pointed at, touched or
/// focused (the header's bell), or with the app when a visit starts here
/// (`index.html`'s early parts). The static copy says where they are.
pub fn page() -> AnyView {
    #[cfg(not(feature = "static-host"))]
    {
        crate::split::page(with_worker::notifications_page()).into_any()
    }
    #[cfg(feature = "static-host")]
    {
        view! { <StaticNote/> }.into_any()
    }
}

#[cfg(feature = "static-host")]
#[component]
fn StaticNote() -> impl IntoView {
    crate::util::set_title("Notifications");
    view! {
        <section class="notifications">
            <h1 class="page-title">"Notifications"</h1>
            <p class="notif-empty">
                "Notifications need Allthing's server and a sign-in, which this copy of the site doesn't have. They're at "
                <a href="https://allthing.social/notifications">"allthing.social/notifications"</a>"."
            </p>
        </section>
    }
}

#[cfg(not(feature = "static-host"))]
pub mod with_worker {
    use std::collections::{BTreeMap, HashSet};
    use std::sync::Arc;

    use allthing_core::notifications::{
        Group, Kind, NOTIFICATIONS_PATH, NotificationsPayload, Opens, Tab, notifications_path,
    };
    use allthing_core::profile::UserCard;
    use leptos::prelude::*;
    use leptos::task::spawn_local;
    use leptos_router::NavigateOptions;

    use crate::api;
    use crate::components::chips::{Avatar, TimeAgo};
    use crate::components::notifications::use_notifications;
    use crate::components::profile_card::ProfileHover;
    use crate::components::signin::use_session;
    use crate::components::text::ExcerptText;
    use crate::nav::card_click;
    use crate::util::{
        cast_href, element_by_id, notifications_href, set_title, use_go, use_query_param, user_href, with_query,
    };

    #[leptos::wasm_split::wasm_split(
        notifications,
        wasm_split_path = ::leptos::wasm_split,
        fallible,
        preload(preload_notifications)
    )]
    pub fn notifications_page() -> Result<AnyView, leptos::wasm_split::SplitLoaderError> {
        Ok(view! { <NotificationsPage/> }.into_any())
    }

    const PANEL: &str = "notifications-panel";

    /// The tab `?tab=` chooses (tracked).
    fn chosen(tab: Memo<Option<String>>) -> Tab {
        tab.with(|tab| Tab::parse(tab.as_deref()))
    }

    fn tab_id(tab: Tab) -> String {
        format!("notifications-tab-{}", tab.label().to_ascii_lowercase())
    }

    fn tab_href(tab: Tab) -> String {
        with_query(&notifications_href(), &[("tab", tab.query().map(str::to_owned))])
    }

    #[component]
    fn NotificationsPage() -> impl IntoView {
        set_title("Notifications");
        let tab = use_query_param("tab");
        let session = use_session();
        let whose = Memo::new(move |_| session.user.with(|user| user.as_ref().map(|user| user.fid)));
        let signed_in = move || whose.with(Option::is_some);
        view! {
            <section class="notifications">
                <h1 class="page-title">"Notifications"</h1>
                {move || signed_in().then(|| view! { <Tabs tab/> })}
                <div
                    class="notif-panel"
                    id=PANEL
                    role=move || signed_in().then_some("tabpanel")
                    aria-labelledby=move || signed_in().then(|| tab_id(chosen(tab)))
                >
                    // Each account its own list: nothing of one reader's
                    // shows for the next to sign in here.
                    {move || match whose.get() {
                        Some(fid) => view! { <List tab fid/> }.into_any(),
                        None => view! {
                            <div class="notif-empty">
                                <p>"Sign in to see your notifications."</p>
                                <button type="button" class="button" on:click=move |_| session.sign_in()>"Sign in"</button>
                            </div>
                        }
                        .into_any(),
                    }}
                </div>
            </section>
        }
    }

    /// All, Mentions and Replies: links, kept in the URL, as a tab list. The
    /// arrow keys, Home and End move between them, as in any tab list.
    #[component]
    fn Tabs(tab: Memo<Option<String>>) -> impl IntoView {
        let go = use_go();
        let keydown = move |ev: leptos::ev::KeyboardEvent| {
            let now = untrack(|| chosen(tab));
            let at = Tab::ALL.iter().position(|t| *t == now).unwrap_or(0);
            let last = Tab::ALL.len() - 1;
            let to = match ev.key().as_str() {
                "ArrowRight" => (at + 1) % Tab::ALL.len(),
                "ArrowLeft" => (at + last) % Tab::ALL.len(),
                "Home" => 0,
                "End" => last,
                _ => return,
            };
            ev.prevent_default();
            let to = Tab::ALL[to];
            go(&tab_href(to), NavigateOptions { scroll: false, ..Default::default() });
            request_animation_frame(move || {
                if let Some(el) = element_by_id::<web_sys::HtmlElement>(&tab_id(to)) {
                    let _ = el.focus();
                }
            });
        };
        view! {
            <div class="tabs notif-tabs" role="tablist" aria-label="Notifications" on:keydown=keydown>
                {Tab::ALL
                    .into_iter()
                    .map(|t| {
                        let this = move || chosen(tab) == t;
                        view! {
                            <a
                                role="tab"
                                id=tab_id(t)
                                href=tab_href(t)
                                data-noscroll=""
                                aria-controls=PANEL
                                aria-selected=move || this().to_string()
                                tabindex=move || if this() { "0" } else { "-1" }
                                class:selected=this
                            >
                                {t.label()}
                            </a>
                        }
                    })
                    .collect_view()}
            </div>
        }
    }

    /// Rows of placeholders, as tall as rows, while the data comes.
    #[component]
    fn Skeletons() -> impl IntoView {
        view! {
            <ol class="notif-list" aria-busy="true" aria-label="Loading notifications">
                {(0..6)
                    .map(|i| view! {
                        <li class="notif skeleton" style=format!("--i: {i}")>
                            <span class="notif-icon"></span>
                            <div class="notif-body">
                                <div class="notif-top">
                                    <span class="notif-faces">
                                        <span class="sk sk-face"></span>
                                        <span class="sk sk-face"></span>
                                        <span class="sk sk-face"></span>
                                    </span>
                                </div>
                                <span class="sk sk-line"></span>
                                <span class="sk sk-line short"></span>
                            </div>
                        </li>
                    })
                    .collect_view()}
            </ol>
        }
    }

    /// What tells one group from another on the page: its kind and cast
    /// (or day), and its newest time and count, so one that grew is drawn
    /// anew.
    fn key(group: &Group) -> String {
        let of = group.cast.as_ref().map_or_else(|| group.ts.div_euclid(86_400).to_string(), |c| c.hash.clone());
        format!("{:?}:{of}:{}:{}", group.kind, group.ts, group.count)
    }

    /// The notifications of `fid`, the reader signed in.
    #[component]
    fn List(tab: Memo<Option<String>>, fid: u64) -> impl IntoView {
        let Some(notifications) = use_notifications() else { return ().into_any() };
        // What the reader had seen before this visit: anything newer stays
        // marked new until they leave.
        let before = notifications.seen_by(fid);
        // The first page, read whole from the answer the bell read its
        // times from (a read within a minute is reused), and again as they
        // change, until more are loaded after it; then the pages stay as
        // they are, so none slips between two.
        let first = RwSignal::new(None::<Arc<NotificationsPayload>>);
        let more = RwSignal::new(Vec::<Arc<NotificationsPayload>>::new());
        let read_failed = RwSignal::new(None::<api::Failure>);
        Effect::new(move |_| {
            if notifications.times().is_none() || !notifications.are_for(fid) || !more.with_untracked(Vec::is_empty) {
                return;
            }
            spawn_local(async move {
                let read = api::get::<NotificationsPayload>(NOTIFICATIONS_PATH).await;
                // Signed out, or someone else, meanwhile.
                if !notifications.are_for(fid) {
                    return;
                }
                match read {
                    Ok(fetched) if more.with_untracked(Vec::is_empty) => first.set(Some(fetched.data)),
                    Ok(_) => {}
                    Err(failure) => read_failed.set(Some(failure)),
                }
            });
        });
        // Seen, up to the newest on the page: the bell clears.
        Effect::new(move |_| {
            if let Some(page) = first.get() {
                notifications.mark_seen(fid, page.newest);
            }
        });
        notifications.refresh();
        let pages = Memo::new(move |_| first.get().into_iter().chain(more.get()).collect::<Vec<_>>());
        // Whether the first page is in: a newer copy of it updates the rows
        // in place rather than drawing the list anew.
        let loaded = Memo::new(move |_| first.with(Option::is_some));
        // The people's profile cards, from every page.
        let cards = Memo::new(move |_| {
            pages.with(|pages| {
                let mut cards: BTreeMap<u64, UserCard> = BTreeMap::new();
                for page in pages {
                    api::remember_cards(&page.notes.users);
                    cards.extend(page.notes.users.clone());
                }
                cards
            })
        });
        let shown = Memo::new(move |_| {
            let tab = chosen(tab);
            let mut keys = HashSet::new();
            pages.with(|pages| {
                pages
                    .iter()
                    .flat_map(|page| page.groups.iter())
                    .filter(|group| tab.shows(group) && keys.insert(key(group)))
                    .cloned()
                    .collect::<Vec<Group>>()
            })
        });
        let next = Memo::new(move |_| pages.with(|pages| pages.last().and_then(|page| page.next.clone())));
        let loading = RwSignal::new(false);
        let failed_more = RwSignal::new(None::<String>);
        let load_more = move |_| {
            let Some(cursor) = next.get_untracked() else { return };
            loading.set(true);
            failed_more.set(None);
            spawn_local(async move {
                let read = api::get::<NotificationsPayload>(&notifications_path(Some(&cursor))).await;
                if !notifications.are_for(fid) {
                    return;
                }
                match read {
                    Ok(fetched) => more.update(|pages| pages.push(fetched.data)),
                    Err(failure) => failed_more.set(Some(failure.message())),
                }
                loading.set(false);
            });
        };
        (move || {
            if !loaded.get() {
                return match notifications.failed().or_else(|| read_failed.get()) {
                    Some(failure) => view! {
                        <div class="notif-empty notif-error" role="alert">
                            <p>{failure.message()}</p>
                            <button
                                type="button"
                                class="button"
                                on:click=move |_| {
                                    read_failed.set(None);
                                    notifications.retry();
                                }
                            >
                                "Try again"
                            </button>
                        </div>
                    }
                    .into_any(),
                    None => view! { <Skeletons/> }.into_any(),
                };
            }
            view! {
                {move || shown.with(Vec::is_empty).then(|| view! { <p class="notif-empty">{chosen(tab).empty()}</p> })}
                <ol class="notif-list">
                    <For each=move || shown.get() key=key let:group>
                        <Row new={group.ts > before} cards group/>
                    </For>
                </ol>
                {move || next.get().map(|_| view! {
                    <div class="notif-more">
                        <button type="button" class="button notif-more-button" disabled=loading on:click=load_more>
                            {move || if loading.get() { "Loading…" } else { "Load more" }}
                        </button>
                        {move || failed_more.get().map(|why| view! { <p class="notif-error small" role="alert">{why}</p> })}
                    </div>
                })}
            }
            .into_any()
        })
        .into_any()
    }

    const HEART: &str = "M8 13.5S2.5 10 2.5 6.2A2.8 2.8 0 0 1 8 5a2.8 2.8 0 0 1 5.5 1.2C13.5 10 8 13.5 8 13.5z";
    const RECAST: &str = "M4 5.5h7l-1.8-1.8M12 10.5H5l1.8 1.8M4 5.5v3M12 10.5v-3";
    const BUBBLE: &str = "M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z";
    const AT: &str =
        "M10.4 8a2.4 2.4 0 1 1-4.8 0a2.4 2.4 0 0 1 4.8 0zm0 0v1.1a1.5 1.5 0 0 0 3 0V8a5.4 5.4 0 1 0-2.1 4.3";
    const QUOTE: &str = "M3.5 11.5h3v-3H4.8c0-1.6.5-2.6 1.7-3.3M9.5 11.5h3v-3h-1.7c0-1.6.5-2.6 1.7-3.3";
    const FOLLOW: &str = "M6.5 7.3a2.3 2.3 0 1 0 0-4.6a2.3 2.3 0 0 0 0 4.6zM2.2 13.2c.5-2.3 2.2-3.5 4.3-3.5s3.8 1.2 4.3 3.5M12.6 5.3v4M10.6 7.3h4";

    fn look(kind: Kind) -> (&'static str, &'static str) {
        match kind {
            Kind::Like => (HEART, "like"),
            Kind::Recast => (RECAST, "recast"),
            Kind::Reply => (BUBBLE, "reply"),
            Kind::Mention => (AT, "mention"),
            Kind::Quote => (QUOTE, "quote"),
            Kind::Follow => (FOLLOW, "follow"),
        }
    }

    /// One group: what happened, by whom (up to five faces, each opening
    /// that person's profile card), the cast it's about, and when. The row
    /// opens the cast, or for follows the newest follower's profile.
    #[component]
    fn Row(group: Group, new: bool, cards: Memo<BTreeMap<u64, UserCard>>) -> impl IntoView {
        let (icon, kind) = look(group.kind);
        let href = match group.opens() {
            Some(Opens::Cast(hash)) => cast_href(hash),
            Some(Opens::Profile(username)) => user_href(username),
            None => notifications_href(),
        };
        let excerpt = group.cast.as_ref().map(|cast| cast.excerpt.clone()).filter(|text| !text.is_empty());
        let what = match &excerpt {
            Some(text) => format!("{}: {text}", group.sentence()),
            None => group.sentence(),
        };
        let label = if new { format!("New: {what}") } else { what };
        let first = group.first().cloned();
        let card = first.as_ref().and_then(|person| cards.with_untracked(|c| c.get(&person.fid).cloned()));
        let faces = group.actors.clone();
        let open = card_click(href.clone());
        view! {
            <li class="notif" class:new=new data-kind=kind on:click=open>
                <span class=format!("notif-icon notif-{kind}") aria-hidden="true">
                    <svg viewBox="0 0 16 16"><path d=icon/></svg>
                </span>
                <div class="notif-body">
                    <div class="notif-top">
                        <div class="notif-faces">
                            {faces
                                .into_iter()
                                .map(|person| {
                                    let card = cards.with_untracked(|c| c.get(&person.fid).cloned());
                                    let (name, link, pfp) =
                                        (person.name().to_owned(), user_href(&person.username), person.pfp_url.clone());
                                    view! {
                                        <ProfileHover user=person card class="notif-face-hover">
                                            <a class="notif-face" href=link.clone() aria-label=name.clone()>
                                                <Avatar url=pfp.clone() size=28/>
                                            </a>
                                        </ProfileHover>
                                    }
                                })
                                .collect_view()}
                        </div>
                        <span class="notif-when muted small"><TimeAgo ts=group.ts/></span>
                    </div>
                    <p class="notif-line">
                        {first.map(|person| {
                            let link = user_href(&person.username);
                            let name = person.name().to_owned();
                            view! {
                                <ProfileHover user=person card class="notif-who">
                                    <a class="notif-name" href=link.clone()>{name.clone()}</a>
                                </ProfileHover>
                            }
                        })}
                        <a class="notif-open" href=href aria-label=label>{group.rest()}</a>
                    </p>
                    {excerpt.map(|text| view! { <p class="notif-excerpt"><ExcerptText text/></p> })}
                </div>
            </li>
        }
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! An account's followers, and whom it follows (`/u/<name>/followers`,
//! `/u/<name>/following`): the newest follow first, as the Farcaster app
//! lists them, a page of people at a time (`/api/v1/follows`).
//!
//! The list is as tall as all its people from the start, and only the rows
//! in view and a few either side are drawn (`window`), so a list of
//! thousands scrolls as one of a screen. The next page is read as the
//! reader comes within two screens of the end (`loader`), and what a visit
//! loaded of a list is kept, so Back to it shows it whole, where it was
//! (`store`). Its code is a part of its own (`crate::split`).

mod loader;
mod row;
mod store;
mod window;

use allthing_core::api::{FollowsDir, FollowsOwner, UserPayload, user_path};
use allthing_core::format::full_number;
use allthing_core::params::username_from_param;
use leptos::html::Ol;
use leptos::prelude::*;
use leptos::wasm_split::{SplitLoaderError, wasm_split};
use leptos_router::hooks::{use_location, use_params_map};

use self::loader::Loader;
use self::row::PersonRow;
use self::store::People;
use self::window::Window;
use crate::api::{self, Failure};
use crate::components::chips::Avatar;
use crate::components::layout::{ErrorPanel, NotFound};
use crate::components::people::{entrance, waiting_rows};
use crate::util::{followers_href, following_href, set_title, user_href};

/// The page, whose code is fetched the first time it's needed
/// (`crate::split`): as it opens, or ahead of it.
#[wasm_split(follows, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_follows))]
pub fn follows_page() -> Result<AnyView, SplitLoaderError> {
    Ok(view! { <FollowsPage/> }.into_any())
}

#[component]
fn FollowsPage() -> impl IntoView {
    let params = use_params_map();
    let location = use_location();
    let name = Memo::new(move |_| params.read().get("name").and_then(|raw| username_from_param(&raw)));
    let dir = Memo::new(move |_| match location.pathname.read().ends_with("/following") {
        true => FollowsDir::Following,
        false => FollowsDir::Followers,
    });
    move || match name.get() {
        Some(name) => view! { <FollowsList name dir=dir.get()/> }.into_any(),
        None => view! { <NotFound/> }.into_any(),
    }
}

/// What the page shows under its header and toolbar.
#[derive(Clone, PartialEq)]
enum Shown {
    /// The first page is on its way.
    Waiting,
    Failed(Failure),
    /// The list has no one.
    Empty,
    Listed,
}

#[component]
fn FollowsList(name: String, dir: FollowsDir) -> impl IntoView {
    let people = People::new((name.clone(), dir), preview(&name));
    let loader = Loader::new(people, name.clone(), dir);
    Effect::new(move |_| {
        if let Some(owner) = people.owner.get() {
            set_title(&title(&owner, dir));
        }
    });
    let shown = Memo::new(move |_| match (people.failed.get(), people.loaded.get()) {
        (Some(failure), _) => Shown::Failed(failure),
        (None, false) => Shown::Waiting,
        (None, true) if people.complete() && people.count() == 0 => Shown::Empty,
        (None, true) => Shown::Listed,
    });
    // No such account: nothing of the page but saying so.
    let missing = Memo::new(move |_| matches!(shown.get(), Shown::Failed(Failure::NotFound(_))));
    view! {
        <section class="follows-page" style=entrance()>
            {move || (!missing.get()).then(|| view! {
                <h1 class="visually-hidden">{move || people.owner.get().map(|owner| title(&owner, dir))}</h1>
                <Head name=name.clone() dir owner=people.owner/>
                <Toolbar dir owner=people.owner/>
            })}
            {move || match shown.get() {
                Shown::Waiting => waiting_rows(people.owner.get_untracked().map(|owner| owner.count(dir))).into_any(),
                Shown::Failed(Failure::NotFound(_)) => view! { <NotFound/> }.into_any(),
                Shown::Failed(failure) => view! { <ErrorPanel failure/> }.into_any(),
                Shown::Empty => view! {
                    <p class="empty follows-empty">
                        {match dir {
                            FollowsDir::Followers => "No followers yet",
                            FollowsDir::Following => "Not following anyone yet",
                        }}
                    </p>
                }
                .into_any(),
                Shown::Listed => view! { <List people loader dir/> }.into_any(),
            }}
        </section>
    }
}

/// "People following Alice (@alice)", "People Alice (@alice) follows".
fn title(owner: &FollowsOwner, dir: FollowsDir) -> String {
    match dir {
        FollowsDir::Followers => format!("People following {} (@{})", owner.name(), owner.username),
        FollowsDir::Following => format!("People {} (@{}) follows", owner.name(), owner.username),
    }
}

/// The account as far as the visit knows it, for the header before the
/// first page: the profile the reader came from, either of its lists, or
/// the author of a cast a feed showed, with its card's counts.
fn preview(name: &str) -> Option<FollowsOwner> {
    if let Some(owner) = store::kept_owner(name) {
        return Some(owner);
    }
    if let Some(Ok(profile)) = api::peek::<UserPayload>(&user_path(name)) {
        let user = &profile.data.user;
        return Some(FollowsOwner {
            fid: user.fid,
            username: user.username.clone(),
            display_name: user.display_name.clone(),
            pfp_url: user.pfp_url.clone(),
            followers: user.follower_count,
            following: user.following_count,
        });
    }
    let seen = api::seen_person(name).filter(|preview| preview.has_card)?;
    let user = seen.user;
    Some(FollowsOwner {
        fid: user.fid,
        username: user.username,
        display_name: user.display_name,
        pfp_url: user.pfp_url,
        followers: user.follower_count,
        following: user.following_count,
    })
}

/// The account, a way back to its profile, and the two lists as tabs.
#[component]
fn Head(name: String, dir: FollowsDir, owner: RwSignal<Option<FollowsOwner>>) -> impl IntoView {
    let location = use_location();
    // Switching lists keeps what the address asks of them.
    let tab = move |to: FollowsDir| {
        let path = match to {
            FollowsDir::Followers => followers_href(&name),
            FollowsDir::Following => following_href(&name),
        };
        let search = location.search.get();
        if search.is_empty() { path } else { format!("{path}?{search}") }
    };
    let count = move |of: FollowsDir| {
        owner.with(|owner| match owner {
            Some(owner) => view! { <span class="num">{full_number(owner.count(of))}</span> }.into_any(),
            None => view! { <span class="num sk-num" aria-hidden="true"></span> }.into_any(),
        })
    };
    let tabs = [FollowsDir::Followers, FollowsDir::Following]
        .into_iter()
        .map(|of| {
            let selected = of == dir;
            let tab = tab.clone();
            view! {
                <a href=move || tab(of) class:selected=selected aria-current=selected.then_some("page")>
                    {match of {
                        FollowsDir::Followers => "Followers",
                        FollowsDir::Following => "Following",
                    }}
                    " "
                    {move || count(of)}
                </a>
            }
        })
        .collect_view();
    view! {
        <div class="follows-head">
            {move || match owner.get() {
                Some(owner) => view! {
                    <a class="follows-owner" href=user_href(&owner.username)>
                        <svg class="icon follows-back" viewBox="0 0 16 16" aria-hidden="true"><path d="M10 3L5 8l5 5"/></svg>
                        <span class="visually-hidden">"Back to "</span>
                        <Avatar url=owner.pfp_url.clone() size=40/>
                        <span class="follows-owner-names">
                            <strong>{owner.name().to_owned()}</strong>
                            <span class="muted">"@"{owner.username.clone()}</span>
                        </span>
                    </a>
                }
                .into_any(),
                None => view! {
                    <span class="follows-owner" aria-hidden="true">
                        <span class="sk sk-avatar"></span>
                        <span class="follows-owner-names"><span class="sk sk-title"></span><span class="sk sk-meta"></span></span>
                    </span>
                }
                .into_any(),
            }}
            <nav class="tabs follows-tabs" aria-label="Followers and following">{tabs}</nav>
        </div>
    }
}

/// Searching and sorting the list: in place from the first frame, so the
/// list doesn't move when they come.
#[component]
fn Toolbar(dir: FollowsDir, owner: RwSignal<Option<FollowsOwner>>) -> impl IntoView {
    let noun = dir.as_str();
    let placeholder = move || match owner.get() {
        Some(owner) => format!("Search {} {noun}", full_number(owner.count(dir))),
        None => format!("Search {noun}"),
    };
    view! {
        <div class="follows-toolbar">
            <div class="follows-search">
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5L14 14"/></svg>
                <input type="search" placeholder=placeholder aria-label=format!("Search {noun}") disabled=true/>
            </div>
            <button type="button" class="follows-sort" disabled=true>
                "Recent"
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d="M4 6l4 4 4-4"/></svg>
            </button>
        </div>
    }
}

/// The people, a window of rows at a time, and what comes after them.
#[component]
fn List(people: People, loader: Loader, dir: FollowsDir) -> impl IntoView {
    let list = NodeRef::<Ol>::new();
    let count = Signal::derive(move || people.count());
    let window = Window::new(list, count);
    // Within two screens of the end: the next page.
    Effect::new(move |_| {
        let (_, to) = window.range.get();
        let n = people.count();
        if n.saturating_sub(to) <= 2 * window.screen.get() {
            loader.want(n + 1);
        }
    });
    // How many the list holds: all of them once all are in; until then as
    // the account's profile counts them, or not known.
    let setsize = Signal::derive(move || {
        let n = people.count() as i64;
        if people.complete() {
            return n;
        }
        people.owner.with(|owner| owner.as_ref().map(|o| o.count(dir) as i64).filter(|&total| total >= n).unwrap_or(-1))
    });
    let label = match dir {
        FollowsDir::Followers => "Followers",
        FollowsDir::Following => "Following",
    };
    view! {
        <ol class="people-list" node_ref=list aria-label=label style:height=move || format!("{}px", count.get() as f64 * window.row.get())>
            <For
                each=move || {
                    let (from, to) = window.range.get();
                    (from..to).collect::<Vec<usize>>()
                }
                key=|index| *index
                children=move |index| {
                    people.people.with_untracked(|all| all.get(index).cloned()).map(|person| {
                        view! { <PersonRow person index row=window.row setsize/> }
                    })
                }
            />
        </ol>
        <div class="follows-tail" aria-live="polite">
            {move || {
                if loader.stuck.get() {
                    view! {
                        <p class="follows-more">
                            "Couldn't load more people. "
                            <button type="button" class="link-button" on:click=move |_| loader.retry()>"Try again"</button>
                        </p>
                    }
                    .into_any()
                } else if loader.loading.get() {
                    view! { <p class="follows-more muted">"Loading more…"</p> }.into_any()
                } else {
                    ().into_any()
                }
            }}
        </div>
    }
}

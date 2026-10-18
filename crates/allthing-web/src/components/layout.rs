// SPDX-License-Identifier: AGPL-3.0-or-later
//! Page frame: header with the wordmark, footer, sidebar boxes, sort tabs,
//! pagination and loading or error states.

use allthing_core::ranking::Sort;
use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::api::Failure;
use crate::components::search::SearchBox;
use crate::util::{BASE, SOURCE_URL, href, with_query};

#[component]
pub fn Wordmark() -> impl IntoView {
    view! { <a class="wordmark" href=href("/") aria-label="Allthing, home">"Allthing"</a> }
}

#[component]
pub fn SiteHeader() -> impl IntoView {
    let location = use_location();
    let current = move |prefix: &'static str| {
        let path = location.pathname.get();
        let path = path.strip_prefix(BASE).unwrap_or(&path);
        if prefix == "/" { path == "/" || path.is_empty() } else { path.starts_with(prefix) }
    };
    view! {
        <header class="site-header">
            <div class="header-inner">
                <Wordmark/>
                <nav class="main-nav" aria-label="Main">
                    <a href=href("/") aria-current=move || current("/").then_some("page")>"Home"</a>
                </nav>
                <SearchBox/>
                <span class="readonly-pill" title="Allthing only reads public Farcaster data. There is no sign-in and nothing is ever posted.">"read-only"</span>
            </div>
            <div class="portal-line" aria-hidden="true"></div>
        </header>
    }
}

#[component]
pub fn SiteFooter() -> impl IntoView {
    view! {
        <footer class="site-footer">
            <a href=format!("{SOURCE_URL}/blob/main/LICENSE")>"AGPL-3.0"</a>
            <span aria-hidden="true">" · "</span>
            <a href=SOURCE_URL>"Source"</a>
            <span aria-hidden="true">" · "</span>
            <span class="version" title=format!("Allthing {} built from commit {}", crate::VERSION, crate::COMMIT)>
                {format!("v{} ({})", crate::VERSION, crate::COMMIT)}
            </span>
        </footer>
    }
}

#[component]
pub fn SideBox(#[prop(into)] title: String, children: Children) -> impl IntoView {
    view! {
        <section class="side-box">
            <h2>{title}</h2>
            {children()}
        </section>
    }
}

/// Main column plus sidebar. `class` adds a modifier, such as
/// `profile-columns`, which moves the sidebar under the profile header on
/// narrow screens.
#[component]
pub fn Columns(
    /// Sidebar content; without it the page is one centered column.
    #[prop(optional)]
    side: Option<AnyView>,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let single = if side.is_none() { " single" } else { "" };
    view! {
        <div class=format!("columns{single} {class}")>
            <div class="content">{children()}</div>
            {side.map(|side| view! { <aside class="side" aria-label="Sidebar">{side}</aside> })}
        </div>
    }
}

/// hot · new · top links for a listing at `path`.
#[component]
pub fn SortTabs(path: String, current: Sort, #[prop(optional)] default_sort: Option<Sort>) -> impl IntoView {
    let default_sort = default_sort.unwrap_or(Sort::Hot);
    view! {
        <nav class="sort-tabs" aria-label="Sort">
            {Sort::ALL
                .into_iter()
                .map(|sort| {
                    let href = with_query(&path, &[("sort", (sort != default_sort).then(|| sort.as_str().to_owned()))]);
                    view! {
                        <a href=href data-noscroll="" class:selected=sort == current aria-current=(sort == current).then_some("page") title=sort.describe()>
                            {sort.as_str()}
                        </a>
                    }
                })
                .collect_view()}
        </nav>
    }
}

/// Says so when a page shows a cached copy because the data source isn't
/// answering; nothing otherwise.
#[component]
pub fn StaleNotice(stale: bool) -> impl IntoView {
    stale.then(|| {
        view! {
            <p class="stale-notice" role="status">
                "Showing a cached copy: the Farcaster data source is not answering right now."
            </p>
        }
    })
}

#[component]
pub fn ErrorPanel(failure: Failure) -> impl IntoView {
    let (title, retry) = match &failure {
        Failure::NotFound(_) => ("Nothing here", false),
        Failure::Busy { .. } => ("The data source is busy", true),
        Failure::Failed(_) => ("Something went wrong", true),
    };
    view! {
        <section class="error-panel" role="alert">
            <h1>{title}</h1>
            <p>{failure.message()}</p>
            {retry.then(|| view! { <button type="button" class="button" on:click=move |_| { let _ = window().location().reload(); }>"Try again"</button> })}
            <p><a href=href("/")>"Back to the front page"</a></p>
        </section>
    }
}

#[component]
pub fn NotFound() -> impl IntoView {
    crate::util::set_title("Not found");
    view! {
        <section class="error-panel not-found">
            <h1>"Page not found"</h1>
            <p>"This corner of the Allthing is empty."</p>
            <p><a href=href("/")>"Back to the front page"</a></p>
        </section>
    }
}

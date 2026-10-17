// SPDX-License-Identifier: AGPL-3.0-or-later
//! Page frame: header with the wordmark, footer, sidebar boxes, sort tabs,
//! pagination and loading or error states.

use allthing_core::listing::PageSlice;
use allthing_core::ranking::Sort;
use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::api::Failure;
use crate::util::{SOURCE_URL, with_query};

#[component]
pub fn Wordmark() -> impl IntoView {
    view! { <a class="wordmark" href="/" aria-label="Allthing, front page">"Allthing"</a> }
}

#[component]
pub fn SiteHeader() -> impl IntoView {
    let location = use_location();
    let current = move |prefix: &'static str| {
        let path = location.pathname.get();
        if prefix == "/" { path == "/" } else { path.starts_with(prefix) }
    };
    view! {
        <header class="site-header">
            <div class="header-inner">
                <Wordmark/>
                <nav class="main-nav" aria-label="Main">
                    <a href="/" aria-current=move || current("/").then_some("page")>"Front page"</a>
                    <a href="/channels" aria-current=move || current("/channels").then_some("page")>"Channels"</a>
                    <a href="/about" aria-current=move || current("/about").then_some("page")>"About"</a>
                </nav>
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
            <p>
                <strong>"Allthing"</strong>
                " is free software under the "
                <a href=format!("{SOURCE_URL}/blob/main/LICENSE")>"GNU AGPL v3 or later"</a>
                ". "
                <a href=SOURCE_URL>"Source code"</a>
                " · Farcaster data via Hypersnap · Neynar scores read onchain on Base · spam labels from the Farcaster app's public dataset."
            </p>
            <p class="fine">"Independent and read-only; not affiliated with Farcaster, Neynar or Quilibrium. The name nods to the Allthing of Dan Simmons' Hyperion Cantos."</p>
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

/// Main column plus sidebar.
#[component]
pub fn Columns(side: AnyView, children: Children) -> impl IntoView {
    view! {
        <div class="columns">
            <div class="content">{children()}</div>
            <aside class="side" aria-label="Sidebar">{side}</aside>
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
                        <a href=href class:selected=sort == current aria-current=(sort == current).then_some("page") title=sort.describe()>
                            {sort.as_str()}
                        </a>
                    }
                })
                .collect_view()}
        </nav>
    }
}

/// Previous and next page links for a paginated listing.
#[component]
pub fn Pager(
    path: String,
    slice: PageSlice,
    #[prop(optional)] sort: Option<Sort>,
    #[prop(optional)] default_sort: Option<Sort>,
) -> impl IntoView {
    let sort_param = sort.filter(|s| Some(*s) != default_sort.or(Some(Sort::Hot))).map(|s| s.as_str().to_owned());
    let link =
        |page: u32| with_query(&path, &[("sort", sort_param.clone()), ("page", (page > 1).then(|| page.to_string()))]);
    (slice.has_prev || slice.has_next).then(|| {
        view! {
            <nav class="pager" aria-label="Pages">
                {slice.has_prev.then(|| view! { <a rel="prev" href=link(slice.page - 1)>"‹ prev"</a> })}
                <span class="page-number">"page "{slice.page}</span>
                {slice.has_next.then(|| view! { <a rel="next" href=link(slice.page + 1)>"next ›"</a> })}
            </nav>
        }
    })
}

/// Freshness note for a payload served from the edge cache.
#[component]
pub fn Freshness(age: u32, stale: bool) -> impl IntoView {
    let text = match (stale, age) {
        (true, _) => "Showing a cached copy: the Farcaster data source is not answering right now.".to_owned(),
        (false, 0..=59) => "Updated just now".to_owned(),
        (false, 60..=3599) => format!("Updated {} min ago", age / 60),
        (false, _) => format!("Updated {} h ago", age / 3600),
    };
    view! { <p class="freshness" class:stale=stale role=stale.then_some("status")>{text}</p> }
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
            <p><a href="/">"Back to the front page"</a></p>
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
            <p><a href="/">"Back to the front page"</a></p>
        </section>
    }
}

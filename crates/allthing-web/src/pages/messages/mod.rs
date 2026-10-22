// SPDX-License-Identifier: AGPL-3.0-or-later
//! Messages: one inbox of Allthing's own messages, on its SpacetimeDB
//! database (`realtime`, docs/ARCHITECTURE.md#messages-on-spacetimedb): the public
//! `#general` room and end-to-end encrypted conversations, listed together.
//! Nothing here goes to Farcaster, and no API key is asked for. Its code is
//! a part of its own (`crate::split`), fetched as the page opens, or as the
//! account menu's Messages is pointed at. The static copy has no sign-in,
//! so it only points to allthing.social.

use leptos::prelude::*;

use crate::util::set_title;

#[cfg(not(feature = "static-host"))]
mod people;
#[cfg(not(feature = "static-host"))]
mod realtime;

#[component]
pub fn MessagesPage() -> impl IntoView {
    set_title("Messages");
    view! {
        <section class="dm">
            <header class="dm-head">
                <h1 class="page-title">"Messages"</h1>
                <p class="dm-lede muted">"Allthing's room and your encrypted messages. Nothing here is posted to Farcaster."</p>
            </header>
            {inbox()}
        </section>
    }
}

#[cfg(feature = "static-host")]
fn inbox() -> impl IntoView {
    view! {
        <section class="dm-panel">
            <p>
                "Messages need Allthing's sign-in, which this copy of the site doesn't have. They're at "
                <a href="https://allthing.social/messages">"allthing.social/messages"</a>
                "."
            </p>
        </section>
    }
}

/// The inbox, from the part fetched with the page.
#[cfg(not(feature = "static-host"))]
fn inbox() -> impl IntoView {
    crate::split::lazy_page(|| view! { <Waiting text="Loading your messages…"/> }, realtime::inbox())
}

/// The box the page shows its conversations in, empty, while something
/// loads: the same size as when they come, so nothing moves.
#[cfg(not(feature = "static-host"))]
#[component]
fn Waiting(text: &'static str) -> impl IntoView {
    view! {
        <div class="dm-box dm-waiting" aria-busy="true">
            <div class="dm-list"></div>
            <div class="dm-pane"><p class="dm-empty muted" role="status">{text}</p></div>
        </div>
    }
}

/// Start fetching the page's code, once: the account menu's Messages is
/// pointed at, touched or focused.
#[cfg(not(feature = "static-host"))]
pub fn warm() {
    thread_local! {
        static ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if ASKED.with(|asked| asked.replace(true)) {
        return;
    }
    leptos::task::spawn_local(async {
        if realtime::preload_realtime().await.is_err() {
            // Couldn't come: the next intent asks again.
            ASKED.with(|asked| asked.set(false));
        }
    });
}

/// The account menu's link to Messages. Pointing at it, or focusing it,
/// fetches the page's code.
#[cfg(not(feature = "static-host"))]
#[component]
pub fn MessagesMenuItem() -> impl IntoView {
    view! {
        <a
            class="account-link"
            href=crate::util::messages_href()
            on:pointerenter=move |_| warm()
            on:focus=move |_| warm()
            on:touchstart=move |_| warm()
        >
            "Messages"
        </a>
    }
}

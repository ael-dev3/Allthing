// SPDX-License-Identifier: AGPL-3.0-or-later
//! Messages: the reader's Direct Casts (`allthing_core::dc`,
//! docs/DIRECT_CASTS.md), read and sent with a Farcaster API key they make
//! and paste here, which only their browser keeps. Signed out, or without a
//! key, it hands them to the Farcaster app's inbox. Its code is a part of
//! its own (`crate::split`), fetched as the page opens, or as the account
//! menu's Messages is pointed at. The static copy has no sign-in, so it
//! only points to the inbox.

use leptos::prelude::*;

use crate::util::set_title;

#[cfg(not(feature = "static-host"))]
mod people;
#[cfg(not(feature = "static-host"))]
mod transport;
#[cfg(not(feature = "static-host"))]
mod views;

#[component]
pub fn MessagesPage() -> impl IntoView {
    set_title("Messages");
    view! {
        <section class="dm">
            <header class="dm-head">
                <h1 class="page-title">"Messages"</h1>
                <p class="dm-lede muted">"Your Farcaster Direct Casts."</p>
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
                "Direct Casts need Allthing's sign-in, which this copy of the site doesn't have. They're at "
                <a href="https://allthing.social/messages">"allthing.social/messages"</a>
                ", and in the Farcaster app's "
                <a href=allthing_core::dc::INBOX_URL target="_blank" rel="noopener">"inbox"</a>
                "."
            </p>
        </section>
    }
}

/// The conversations and the one open, from the part fetched with the page.
#[cfg(not(feature = "static-host"))]
fn inbox() -> impl IntoView {
    crate::split::lazy_page(|| view! { <Waiting text="Loading your messages…"/> }, views::inbox())
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
        if views::preload_messages().await.is_err() {
            // Couldn't come: the next intent asks again.
            ASKED.with(|asked| asked.set(false));
        }
    });
}

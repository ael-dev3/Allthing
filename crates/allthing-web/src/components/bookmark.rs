// SPDX-License-Identifier: AGPL-3.0-or-later
//! A private saved-post toggle. It needs a session, never a Farcaster signer.

use leptos::prelude::*;

#[cfg(not(feature = "static-host"))]
#[component]
pub fn BookmarkButton(hash: String) -> impl IntoView {
    use crate::components::signin::use_session;
    use crate::library::{SyncStatus, use_library};

    let library = use_library();
    let session = use_session();
    // Split pages can construct a view under a short-lived async owner before
    // mounting it. Follow the mounted control's effect lifetime, as CastCard's
    // viewport observer does, so Back retains a live membership subscription.
    let node = NodeRef::<leptos::html::Button>::new();
    let watched = hash.clone();
    Effect::new(move |_| {
        if node.get().is_some() {
            library.watch(&watched);
        }
    });
    let key = hash.clone();
    let saved = Memo::new(move |_| library.bookmarked(&key));
    let signed_in = Memo::new(move |_| session.user.with(Option::is_some));
    let waiting = Memo::new(move |_| signed_in.get() && saved.get().is_none());
    let message = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        session.user.with(|user| user.as_ref().map(|user| user.fid));
        message.set(None);
    });
    let label = move || if saved.get() == Some(true) { "Remove bookmark" } else { "Save post" };
    view! {
        <span class="bookmark-action">
            <button
                node_ref=node
                type="button"
                class="bookmark-button"
                class:saved=move || saved.get() == Some(true)
                aria-label=label
                aria-description=move || {
                    if waiting.get() && matches!(library.status(), SyncStatus::Offline | SyncStatus::Failed(_)) { "Open Library to load your saved state" }
                    else if waiting.get() { "Loading saved state" }
                    else if saved.get() == Some(true) { "Remove private bookmark" }
                    else { "Save privately to your Library" }
                }
                aria-pressed=move || saved.get().map(|value| value.to_string())
                aria-busy=move || (waiting.get() && matches!(library.status(), SyncStatus::Connecting)).then_some("true")
                disabled=waiting
                on:click=move |event| {
                    event.stop_propagation();
                    if !signed_in.get_untracked() {
                        session.sign_in();
                        return;
                    }
                    message.set(library.toggle_bookmark(&hash).err());
                }
            >
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true">
                    <path d="M4.5 2h7v12L8 11.5 4.5 14z"/>
                </svg>
            </button>
            <span class="bookmark-message" role=move || message.with(|value| value.as_ref().map(|_| "status"))>
                {move || message.get()}
            </span>
        </span>
    }
}

/// The public-only static copy has no private account state or save action.
#[cfg(feature = "static-host")]
#[component]
pub fn BookmarkButton(hash: String) -> impl IntoView {
    let _ = hash;
}

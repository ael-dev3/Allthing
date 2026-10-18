// SPDX-License-Identifier: AGPL-3.0-or-later
//! Who liked or recasted a cast (a short list on hover, the full list in a
//! dialog), and a preview of a cast's replies.
//!
//! Previews whose data is already in the client cache render it in the
//! frame they open, without "Loading…": feeds preload who liked and recasted
//! the cards in view (`crate::preload`), and hovering a card prefetches its
//! post, whose thread holds the replies.

use std::cell::RefCell;
use std::sync::Arc;

use allthing_core::api::{
    CastPayload, REACTIONS_PREVIEW, ReactionKind, ReactionsPayload, RepliesPayload, cast_path, reactions_path,
    replies_path,
};
use allthing_core::embeds::{ImageSize, resized_image_url};
use allthing_core::model::Embed;
use allthing_core::text::truncate_title;
use leptos::ev;
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use wasm_bindgen::JsCast;
use web_sys::HtmlElement;

use crate::api;
use crate::components::chips::{Avatar, TimeAgo};
use crate::util::{cast_href, user_href};

/// The open full who-list, if any: (cast hash, which reactions).
#[derive(Clone, Copy)]
pub struct WhoList(pub RwSignal<Option<(String, ReactionKind)>>);

thread_local! {
    static RETURN_FOCUS: RefCell<Option<HtmlElement>> = const { RefCell::new(None) };
}

impl WhoList {
    pub fn new() -> WhoList {
        WhoList(RwSignal::new(None))
    }

    pub fn open(self, hash: String, kind: ReactionKind) {
        let active = document().active_element();
        // "See everyone" lives in a temporary preview. Return to its count
        // button, which remains on the page after the preview closes.
        let trigger = active
            .as_ref()
            .and_then(|el| el.closest(".popover").ok().flatten())
            .and_then(|root| root.query_selector(".popover-trigger").ok().flatten());
        RETURN_FOCUS.with(|r| *r.borrow_mut() = trigger.or(active).and_then(|el| el.dyn_into().ok()));
        self.0.set(Some((hash, kind)));
    }

    fn close(self) {
        self.0.set(None);
        if let Some(element) = RETURN_FOCUS.with(|r| r.borrow_mut().take()).filter(|el| el.is_connected()) {
            let _ = element.focus();
        }
    }
}

pub fn use_who_list() -> WhoList {
    expect_context::<WhoList>()
}

fn heading(kind: ReactionKind) -> &'static str {
    match kind {
        ReactionKind::Likes => "Liked by",
        ReactionKind::Recasts => "Recasted by",
    }
}

#[component]
fn People(payload: std::sync::Arc<ReactionsPayload>) -> impl IntoView {
    view! {
        <ul class="who-list">
            {payload.users.iter().map(|u| view! {
                <li>
                    <a href=user_href(&u.username)>
                        <Avatar url=u.pfp_url.clone() size=28/>
                        <span class="who-name">{u.name().to_owned()}</span>
                        <span class="muted">"@"{u.username.clone()}</span>
                    </a>
                </li>
            }).collect_view()}
        </ul>
    }
}

/// The ten newest likers or recasters: at once when the feed preloaded
/// them, otherwise loaded when the popover opens.
#[component]
pub fn WhoPreview(hash: String, kind: ReactionKind) -> impl IntoView {
    let who = use_who_list();
    let path = reactions_path(&hash, kind, REACTIONS_PREVIEW);
    // A resource resolves a tick later even from the cache, which would
    // flash "Loading…": a preloaded list is read now instead.
    let ready = api::peek::<ReactionsPayload>(&path);
    let data = ready.is_none().then(|| {
        LocalResource::new(move || {
            let path = path.clone();
            async move { api::get::<ReactionsPayload>(&path).await }
        })
    });
    view! {
        <p class="popover-title">{heading(kind)}</p>
        {move || match ready.clone().or_else(|| data.and_then(|data| data.get())) {
            None => view! { <p class="muted small popover-loading">"Loading…"</p> }.into_any(),
            Some(Err(failure)) => view! { <p class="muted small">{failure.message()}</p> }.into_any(),
            Some(Ok(fetched)) if fetched.data.users.is_empty() => view! { <p class="muted small">"Nobody yet."</p> }.into_any(),
            Some(Ok(fetched)) => {
                let more = fetched.data.more;
                let hash = hash.clone();
                view! {
                    <People payload=fetched.data.clone()/>
                    {more.then(|| view! {
                        <button type="button" class="link-button" on:click=move |_| who.open(hash.clone(), kind)>"See everyone →"</button>
                    })}
                }
                .into_any()
            }
        }}
    }
}

/// The full who-list as a modal dialog.
#[component]
pub fn WhoDialog() -> impl IntoView {
    let who = use_who_list();
    let location = use_location();
    let dialog = NodeRef::<leptos::html::Div>::new();
    let close = move || who.close();
    // A history navigation or a link in the list must not leave the modal
    // covering a different page. Its old opener no longer needs focus.
    Effect::new(move |_| {
        let _ = (location.pathname.get(), location.search.get());
        who.0.set(None);
        RETURN_FOCUS.with(|r| r.borrow_mut().take());
    });
    Effect::new(move |_| {
        let open = who.0.with(Option::is_some);
        if let Some(root) = document().document_element() {
            let _ = root.class_list().toggle_with_force("who-open", open);
        }
    });
    let keys = window_event_listener(ev::keydown, move |ev| {
        if who.0.with_untracked(Option::is_none) {
            return;
        }
        match ev.key().as_str() {
            "Escape" => {
                ev.prevent_default();
                close();
            }
            "Tab" => {
                let Some(root) = dialog.get_untracked() else { return };
                let Ok(nodes) = root.query_selector_all("button, a[href]") else { return };
                let first = nodes.get(0).and_then(|n| n.dyn_into::<HtmlElement>().ok());
                let last = nodes.get(nodes.length().saturating_sub(1)).and_then(|n| n.dyn_into::<HtmlElement>().ok());
                let active = document().active_element();
                let at = |el: &Option<HtmlElement>| {
                    el.as_ref().map(|e| e.unchecked_ref::<web_sys::Element>().clone()) == active
                };
                if ev.shift_key() && at(&first) {
                    ev.prevent_default();
                    if let Some(last) = last {
                        let _ = last.focus();
                    }
                } else if !ev.shift_key() && at(&last) {
                    ev.prevent_default();
                    if let Some(first) = first {
                        let _ = first.focus();
                    }
                }
            }
            _ => {}
        }
    });
    on_cleanup(move || {
        keys.remove();
        RETURN_FOCUS.with(|r| r.borrow_mut().take());
        if let Some(root) = document().document_element() {
            let _ = root.class_list().remove_1("who-open");
        }
    });
    move || {
        who.0.get().map(|(hash, kind)| {
            let path = reactions_path(&hash, kind, 100);
            let data = LocalResource::new(move || {
                let path = path.clone();
                async move { api::get::<ReactionsPayload>(&path).await }
            });
            let close_button = NodeRef::<leptos::html::Button>::new();
            Effect::new(move |_| {
                if let Some(button) = close_button.get() {
                    let _ = button.focus();
                }
            });
            view! {
                <div class="modal-backdrop" on:click=move |ev: ev::MouseEvent| {
                    if ev.target() == ev.current_target() {
                        close();
                    }
                }>
                    <div class="modal who-modal" role="dialog" aria-modal="true" aria-label=heading(kind) node_ref=dialog>
                        <header class="modal-head">
                            <h2>{heading(kind)}</h2>
                            <button type="button" class="modal-close" aria-label="Close" node_ref=close_button on:click=move |_| close()>"×"</button>
                        </header>
                        <div class="modal-body">
                            {move || match data.get() {
                                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                                Some(Err(failure)) => view! { <p class="muted">{failure.message()}</p> }.into_any(),
                                Some(Ok(fetched)) => {
                                    let capped = fetched.data.users.len() >= 100;
                                    view! {
                                        <People payload=fetched.data.clone()/>
                                        {capped.then(|| view! { <p class="muted small">"Showing the 100 newest; Farcaster's read API lists no more."</p> })}
                                    }
                                    .into_any()
                                }
                            }}
                        </div>
                    </div>
                </div>
            }
        })
    }
}

/// A post payload's replies, as the replies preview lists them.
fn replies_of(post: api::Fetched<CastPayload>) -> api::Fetched<RepliesPayload> {
    api::Fetched { data: Arc::new(RepliesPayload::from_post(&post.data)), age: post.age, stale: post.stale }
}

/// A cast's most engaged direct replies. They come from the post's own
/// payload when hovering the card prefetched it (at once, or once that
/// prefetch lands), and are loaded on their own otherwise.
#[component]
pub fn RepliesPreview(hash: String) -> impl IntoView {
    let path = replies_path(&hash);
    let post = cast_path(&hash);
    let ready =
        api::peek::<RepliesPayload>(&path).or_else(|| api::peek::<CastPayload>(&post).map(|r| r.map(replies_of)));
    let data = ready.is_none().then(|| {
        LocalResource::new(move || {
            let (path, post) = (path.clone(), post.clone());
            async move {
                match api::settled::<CastPayload>(&post).await {
                    Some(found) => found.map(replies_of),
                    None => api::get::<RepliesPayload>(&path).await,
                }
            }
        })
    });
    let thread = cast_href(&hash);
    view! {
        <p class="popover-title">"Replies"</p>
        <div class="replies-preview">
            {move || match ready.clone().or_else(|| data.and_then(|data| data.get())) {
                None => view! { <p class="muted small popover-loading">"Loading…"</p> }.into_any(),
                Some(Err(failure)) => view! { <p class="muted small">{failure.message()}</p> }.into_any(),
                Some(Ok(fetched)) if fetched.data.replies.is_empty() => view! { <p class="muted small">"No replies to show."</p> }.into_any(),
                Some(Ok(fetched)) => {
                    let payload = fetched.data.clone();
                    view! {
                        <ul class="reply-items">
                            {payload.replies.iter().map(|reply| {
                                let thumbs: Vec<String> = reply.embeds.iter().filter_map(|e| match e {
                                    Embed::Image { url } => Some(resized_image_url(url, ImageSize::Thumb)),
                                    _ => None,
                                }).take(3).collect();
                                view! {
                                    <li>
                                        <a class="reply-item" href=cast_href(&reply.hash)>
                                            <span class="reply-who">
                                                <Avatar url=reply.author.pfp_url.clone() size=22/>
                                                <strong>{reply.author.name().to_owned()}</strong>
                                                <span class="muted small"><TimeAgo ts=reply.ts/></span>
                                            </span>
                                            <span class="reply-text">{truncate_title(&reply.text, 220)}</span>
                                            {(!thumbs.is_empty()).then(|| view! {
                                                <span class="reply-thumbs">
                                                    {thumbs.into_iter().map(|src| view! { <img src=src alt="" loading="lazy" referrerpolicy="no-referrer"/> }).collect_view()}
                                                </span>
                                            })}
                                        </a>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }
                    .into_any()
                }
            }}
        </div>
        <a class="popover-foot" href=thread>"View full thread →"</a>
    }
}

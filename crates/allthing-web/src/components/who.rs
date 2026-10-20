// SPDX-License-Identifier: AGPL-3.0-or-later
//! Who liked or recasted a cast (a short list on hover, the full list in a
//! dialog), and a preview of a cast's replies.
//!
//! A preview whose data the client cache already holds renders it in the
//! frame it opens, without "Loading…": feeds preload who liked and recasted
//! the cards in view (`crate::preload`), and hovering a card prefetches its
//! post, whose thread holds the replies. Once the reader shows interest in
//! a preview's list, the votes and wallets of the people it names load too,
//! so a card opened from a row is complete at once.

use std::cell::RefCell;
use std::sync::Arc;

use allthing_core::api::{
    CastPayload, REACTIONS_PREVIEW, ReactionKind, ReactionsPayload, RepliesPayload, cast_path, reactions_path,
    replies_path,
};
use allthing_core::embeds::{ImageSize, embed_label, resized_image_url};
use allthing_core::model::Embed;
use allthing_core::text::truncate_title;
use leptos::ev;
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use wasm_bindgen::JsCast;
use web_sys::HtmlElement;

use crate::api;
use crate::components::chips::{Avatar, TimeAgo};
use crate::components::profile_card::ProfileHover;
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

/// People who reacted, each linking to their profile, and each showing the
/// person's profile card: in the full list, in its dialog; in the short
/// list, beside the preview's panel, so the card never covers the names.
/// The short list's people have their votes and wallets preloaded as it
/// opens (`api::prefetch_row_facts`): it opens on a tap, or once the pointer
/// has rested on its count, so a reader means to look at it. The person on a
/// row pointed at or touched goes first (`api::prefetch_facts_first`), in
/// the full list too, whose cards otherwise read theirs when they open.
#[component]
fn People(payload: std::sync::Arc<ReactionsPayload>, #[prop(optional)] cards: bool) -> impl IntoView {
    // Their bios, counts and join dates come with the list: a card opened
    // from it shows them at once.
    api::remember_cards(&payload.cards);
    if !cards {
        api::prefetch_row_facts(&payload.users.iter().map(|u| u.fid).collect::<Vec<_>>());
    }
    view! {
        <ul class="who-list">
            {payload.users.iter().map(|u| {
                let fid = u.fid;
                let href = user_href(&u.username);
                let (pfp, name, username) = (u.pfp_url.clone(), u.name().to_owned(), u.username.clone());
                view! {
                    <li on:pointerenter=move |_| api::prefetch_facts_first(fid) on:focusin=move |_| api::prefetch_facts_first(fid)>
                        <ProfileHover user=u.clone() in_dialog=cards beside=!cards class="who-links">
                            <a class="who-link" href=href.clone()>
                                <Avatar url=pfp.clone() size=28/>
                                <span class="who-name">{name.clone()}</span>
                                <span class="muted">"@"{username.clone()}</span>
                            </a>
                        </ProfileHover>
                    </li>
                }
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
                                        <People payload=fetched.data.clone() cards=true/>
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
                    // The authors' bios, counts and join dates come with the
                    // replies: a card opened from a name shows them at once,
                    // and so do their votes and wallets, once preloaded.
                    api::remember_cards(&payload.notes.users);
                    let mut authors: Vec<u64> = Vec::new();
                    for reply in &payload.replies {
                        if !authors.contains(&reply.author.fid) {
                            authors.push(reply.author.fid);
                        }
                    }
                    api::prefetch_row_facts(&authors);
                    view! {
                        <ul class="reply-items">
                            {payload.replies.iter().map(|reply| {
                                let thumbs: Vec<String> = reply.embeds.iter().filter_map(|e| match e {
                                    Embed::Image { url } => Some(resized_image_url(url, ImageSize::Thumb)),
                                    _ => None,
                                }).take(3).collect();
                                let text = reply.text.trim();
                                // A reply with no text says what it holds, so its row isn't blank.
                                let label = if text.is_empty() { embed_label(&reply.embeds) } else { None };
                                let author = reply.author.clone();
                                let profile = user_href(&author.username);
                                let post = cast_href(&reply.hash);
                                let fid = author.fid;
                                view! {
                                    <li
                                        on:pointerenter=move |_| api::prefetch_facts_first(fid)
                                        on:focusin=move |_| api::prefetch_facts_first(fid)
                                    >
                                        <span class="reply-who">
                                            <ProfileHover user=author.clone() beside=true class="reply-author">
                                                <a class="reply-author-link" href=profile.clone()>
                                                    <Avatar url=author.pfp_url.clone() size=22/>
                                                    <strong>{author.name().to_owned()}</strong>
                                                </a>
                                            </ProfileHover>
                                            <a class="reply-time muted small" href=post.clone()><TimeAgo ts=reply.ts/></a>
                                        </span>
                                        <a class="reply-item" href=post>
                                            {(!text.is_empty()).then(|| view! { <span class="reply-text">{truncate_title(text, 220)}</span> })}
                                            {label.map(|label| view! { <span class="reply-label muted small">{label}</span> })}
                                            {(!thumbs.is_empty()).then(|| view! {
                                                <span class="reply-thumbs">
                                                    // In view as the preview opens: not left to lazy loading,
                                                    // which left them blank there.
                                                    {thumbs.into_iter().map(|src| view! { <img src=src alt="" decoding="async" referrerpolicy="no-referrer"/> }).collect_view()}
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

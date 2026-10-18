// SPDX-License-Identifier: AGPL-3.0-or-later
//! Who liked or recasted a cast (a short list on hover, the full list in a
//! dialog), and a preview of a cast's replies.

use allthing_core::api::{ReactionKind, ReactionsPayload, RepliesPayload, reactions_path, replies_path};
use allthing_core::embeds::{ImageSize, resized_image_url};
use allthing_core::model::Embed;
use allthing_core::text::truncate_title;
use leptos::ev;
use leptos::prelude::*;

use crate::api;
use crate::components::chips::{Avatar, ScoreBadge, TimeAgo};
use crate::util::{cast_href, user_href};

/// The open full who-list, if any: (cast hash, which reactions).
#[derive(Clone, Copy)]
pub struct WhoList(pub RwSignal<Option<(String, ReactionKind)>>);

impl WhoList {
    pub fn new() -> WhoList {
        WhoList(RwSignal::new(None))
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
            {payload.users.iter().map(|u| {
                let score = payload.scores.get(&u.fid).copied().unwrap_or(0);
                view! {
                    <li>
                        <a href=user_href(&u.username)>
                            <Avatar url=u.pfp_url.clone() size=28/>
                            <span class="who-name">{u.name().to_owned()}</span>
                            <span class="muted">"@"{u.username.clone()}</span>
                        </a>
                        <ScoreBadge micros=score/>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
}

/// The ten newest likers or recasters, loaded when the popover opens.
#[component]
pub fn WhoPreview(hash: String, kind: ReactionKind) -> impl IntoView {
    let who = use_who_list();
    let path = reactions_path(&hash, kind, 10);
    let data = LocalResource::new(move || {
        let path = path.clone();
        async move { api::get::<ReactionsPayload>(&path).await }
    });
    view! {
        <p class="popover-title">{heading(kind)}</p>
        {move || match data.get() {
            None => view! { <p class="muted small popover-loading">"Loading…"</p> }.into_any(),
            Some(Err(failure)) => view! { <p class="muted small">{failure.message()}</p> }.into_any(),
            Some(Ok(fetched)) if fetched.data.users.is_empty() => view! { <p class="muted small">"Nobody yet."</p> }.into_any(),
            Some(Ok(fetched)) => {
                let more = fetched.data.more;
                let hash = hash.clone();
                view! {
                    <People payload=fetched.data.clone()/>
                    {more.then(|| view! {
                        <button type="button" class="link-button" on:click=move |_| who.0.set(Some((hash.clone(), kind)))>"See everyone →"</button>
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
    let close = move || who.0.set(None);
    let keys = window_event_listener(ev::keydown, move |ev| {
        if ev.key() == "Escape" && who.0.with_untracked(Option::is_some) {
            close();
        }
    });
    on_cleanup(move || keys.remove());
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
                <div class="modal-backdrop" on:click=move |_| close()>
                    <div class="modal who-modal" role="dialog" aria-modal="true" aria-label=heading(kind)
                        on:click=|ev: ev::MouseEvent| ev.stop_propagation()>
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

/// A cast's most engaged direct replies, loaded when the popover opens.
#[component]
pub fn RepliesPreview(hash: String) -> impl IntoView {
    let path = replies_path(&hash);
    let data = LocalResource::new(move || {
        let path = path.clone();
        async move { api::get::<RepliesPayload>(&path).await }
    });
    let thread = cast_href(&hash);
    view! {
        <p class="popover-title">"Replies"</p>
        <div class="replies-preview">
            {move || match data.get() {
                None => view! { <p class="muted small popover-loading">"Loading…"</p> }.into_any(),
                Some(Err(failure)) => view! { <p class="muted small">{failure.message()}</p> }.into_any(),
                Some(Ok(fetched)) if fetched.data.replies.is_empty() => view! { <p class="muted small">"No replies to show."</p> }.into_any(),
                Some(Ok(fetched)) => {
                    let payload = fetched.data.clone();
                    view! {
                        <ul class="reply-items">
                            {payload.replies.iter().map(|reply| {
                                let score = payload.notes.scores.get(&reply.author.fid).copied().unwrap_or(0);
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
                                                <ScoreBadge micros=score/>
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

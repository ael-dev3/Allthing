// SPDX-License-Identifier: AGPL-3.0-or-later
//! Collapsible reply trees.

use allthing_core::emoji::reactions;
use allthing_core::format::plural;
use allthing_core::model::ThreadNode;
use allthing_core::thread::count_descendants;
use leptos::prelude::*;

use crate::Notes;
use crate::components::chips::{Engagement, OriginChip, TimeAgo, UserChip};
use crate::components::media::{LinkCards, MediaGrid, VideoLinks};
use crate::components::reactions::ReactionChips;
use crate::components::text::CastText;
use crate::util::cast_href;

#[component]
pub fn CommentTree(nodes: Vec<ThreadNode>, notes: Notes, op_fid: u64) -> AnyView {
    nodes.into_iter().map(|node| view! { <Comment node notes=notes.clone() op_fid/> }).collect_view().into_any()
}

/// How long a reply's body animates when it collapses or expands (its CSS
/// transition takes 340 ms).
const TOGGLE_MS: u64 = 400;

#[component]
fn Comment(node: ThreadNode, notes: Notes, op_fid: u64) -> AnyView {
    let collapsed = RwSignal::new(false);
    // The reply is clipped while it collapses or expands; at rest it isn't,
    // so its popovers (likes, profile cards) can reach past its edges.
    let animating = RwSignal::new(false);
    let toggles = StoredValue::new(0_u32);
    let toggle = move |_| {
        collapsed.update(|c| *c = !*c);
        let generation = toggles.get_value().wrapping_add(1);
        toggles.set_value(generation);
        animating.set(true);
        set_timeout(
            move || {
                if toggles.try_get_value() == Some(generation) {
                    let _ = animating.try_set(false);
                }
            },
            std::time::Duration::from_millis(TOGGLE_MS),
        );
    };
    let cast = node.cast.clone();
    let hidden = count_descendants(&node);
    let shown_children = node.children.len() as u64;
    let more = cast.replies.saturating_sub(shown_children);
    let href = cast_href(&cast.hash);
    let gone = crate::components::deleted(&cast.hash);
    let toggle_label = move || {
        if collapsed.get() {
            format!("Expand reply by @{}", node.cast.author.username)
        } else {
            format!(
                "Collapse reply by @{} and its {}",
                node.cast.author.username,
                plural(hidden as u64, "reply", "replies")
            )
        }
    };
    view! {
        <div
            class="comment"
            class:collapsed=move || collapsed.get()
            class:animating=move || animating.get()
            class:deleted=gone
            id=format!("c-{}", &cast.hash[2..10])
        >
            <div class="comment-head">
                // The toggle, who and when, on one line, where a long name
                // gives way to the time.
                <span class="byline">
                    <button
                        class="toggle"
                        type="button"
                        aria-expanded=move || (!collapsed.get()).to_string()
                        aria-label=toggle_label
                        on:click=toggle
                    >
                        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 6l4 4 4-4"/></svg>
                    </button>
                    <UserChip user=cast.author.clone() card=notes.users.get(&cast.author.fid).cloned()/>
                    {(cast.author.fid == op_fid).then(|| view! { <span class="op-mark" aria-description="Original poster">"OP"</span> })}
                    <a class="when" href=href.clone()><span class="visually-hidden">"Open reply, "</span><TimeAgo ts=cast.ts/></a>
                </span>
                <OriginChip hash=cast.hash.clone() notes=notes.clone()/>
                <Show when=move || collapsed.get()>
                    <span class="hidden-count">{format!("{} hidden", plural(hidden as u64 + 1, "reply", "replies"))}</span>
                </Show>
            </div>
            <div class="comment-body" prop:inert=move || collapsed.get()>
                <div class="comment-inner">
                    <p class="deleted-note">"You deleted this reply."</p>
                    {if cast.hidden() {
                        let note = if cast.spam { "Post hidden" } else { "Mini app post hidden" };
                        view! { <p class="mini-app-note">{note}</p> }.into_any()
                    } else {
                        view! { <div class="comment-text"><CastText text=cast.text.clone() mentions=cast.mentions.clone()/></div> }.into_any()
                    }}
                    <MediaGrid cast=cast.clone()/>
                    <VideoLinks cast=cast.clone()/>
                    <LinkCards cast=cast.clone()/>
                    <div class="comment-foot">
                        <Engagement cast=cast.clone() tight=true/>
                        {reply_button(&cast, "reply")}
                    </div>
                    <ReactionChips cast=cast.clone() reacted=reactions(node.children.iter().map(|n| &n.cast))/>
                    {(!node.children.is_empty()).then(|| view! {
                        <div class="children"><CommentTree nodes=node.children.clone() notes=notes.clone() op_fid/></div>
                    })}
                    {(more > 0).then(|| view! {
                        <a class="continue" href=href>{format!("continue this thread ({}) →", plural(more, "more reply", "more replies"))}</a>
                    })}
                </div>
            </div>
        </div>
    }
    .into_any()
}

/// Reply, Quote and React, under a post or a reply, and Delete under the
/// reader's own; `noun` names it in the question ("cast", "reply").
#[cfg(not(feature = "static-host"))]
pub fn reply_button(cast: &allthing_core::model::Cast, noun: &'static str) -> AnyView {
    view! {
        <crate::components::compose::ReplyButton cast=cast.clone()/>
        <crate::components::compose::QuoteButton cast=cast.clone()/>
        <crate::components::reactions::ReactButton cast=cast.clone()/>
        <crate::components::writer::DeleteButton hash=cast.hash.clone() author=cast.author.fid username=cast.author.username.clone() noun/>
    }
    .into_any()
}

/// The static copy has no sign-in, and no Reply.
#[cfg(feature = "static-host")]
pub fn reply_button(_: &allthing_core::model::Cast, _: &'static str) -> AnyView {
    ().into_any()
}

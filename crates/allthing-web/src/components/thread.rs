// SPDX-License-Identifier: AGPL-3.0-or-later
//! Collapsible reply trees.

use allthing_core::format::plural;
use allthing_core::model::ThreadNode;
use allthing_core::thread::count_descendants;
use leptos::prelude::*;

use crate::Notes;
use crate::components::chips::{Engagement, OriginChip, TimeAgo, UserChip};
use crate::components::media::{LinkCards, MediaGrid, VideoLinks};
use crate::components::text::CastText;
use crate::util::cast_href;

#[component]
pub fn CommentTree(nodes: Vec<ThreadNode>, notes: Notes, op_fid: u64) -> AnyView {
    nodes.into_iter().map(|node| view! { <Comment node notes=notes.clone() op_fid/> }).collect_view().into_any()
}

#[component]
fn Comment(node: ThreadNode, notes: Notes, op_fid: u64) -> AnyView {
    let collapsed = RwSignal::new(false);
    let cast = node.cast.clone();
    let hidden = count_descendants(&node);
    let shown_children = node.children.len() as u64;
    let more = cast.replies.saturating_sub(shown_children);
    let href = cast_href(&cast.hash);
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
        <div class="comment" class:collapsed=move || collapsed.get() id=format!("c-{}", &cast.hash[2..10])>
            <div class="comment-head">
                <button
                    class="toggle"
                    type="button"
                    aria-expanded=move || (!collapsed.get()).to_string()
                    aria-label=toggle_label
                    on:click=move |_| collapsed.update(|c| *c = !*c)
                >
                    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 6l4 4 4-4"/></svg>
                </button>
                <UserChip user=cast.author.clone()/>
                {(cast.author.fid == op_fid).then(|| view! { <span class="op-mark" title="Original poster">"OP"</span> })}
                <span class="sep" aria-hidden="true">"·"</span>
                <a class="when" href=href.clone()><TimeAgo ts=cast.ts/></a>
                <OriginChip hash=cast.hash.clone() notes=notes.clone()/>
                <Show when=move || collapsed.get()>
                    <span class="hidden-count">{format!("{} hidden", plural(hidden as u64 + 1, "reply", "replies"))}</span>
                </Show>
            </div>
            <div class="comment-body" prop:inert=move || collapsed.get()>
                <div class="comment-inner">
                    {if cast.hidden() {
                        let note = if cast.spam { "Spam post hidden" } else { "Mini app post hidden" };
                        view! { <p class="mini-app-note">{note}</p> }.into_any()
                    } else {
                        view! { <div class="comment-text"><CastText text=cast.text.clone() mentions=cast.mentions.clone()/></div> }.into_any()
                    }}
                    <MediaGrid cast=cast.clone() open_on_click=true/>
                    <VideoLinks cast=cast.clone()/>
                    <LinkCards cast=cast.clone()/>
                    <div class="comment-foot">
                        <Engagement cast=cast.clone()/>
                        <a class="permalink" href=href.clone()>"permalink"</a>
                    </div>
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

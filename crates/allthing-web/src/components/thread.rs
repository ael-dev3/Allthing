// SPDX-License-Identifier: AGPL-3.0-or-later
//! Collapsible reply trees, with the reader's own replies from here at the
//! top of what they answer: as posting while they're on their way, then as
//! posted, until the page lists them itself.

use std::collections::HashSet;
use std::sync::Arc;

use allthing_core::emoji::reactions;
use allthing_core::format::plural;
use allthing_core::model::{Cast, ThreadNode};
use allthing_core::thread::{collapsed_gap, count_descendants};
use leptos::prelude::*;

use crate::Notes;
use crate::components::cast::CastCard;
use crate::components::chips::{Engagement, OriginChip, TimeAgo, UserChip};
use crate::components::media::{LinkCards, MediaGrid, VideoLinks};
use crate::components::reactions::ReactionChips;
use crate::components::text::CastText;
use crate::util::cast_href;

/// The casts a post's page lists itself, by hash: the reader's replies from
/// here show at the top of what they answer until it lists them.
pub type Listed = Arc<HashSet<String>>;

/// A post's replies (`nodes`, sorted), with the reader's own from here at
/// the top; or, with none, "No replies yet." `rest`, the replies after
/// `nodes`, are drawn once `drawn` is true (at once without it).
#[component]
pub fn Replies(
    parent: String,
    mut nodes: Vec<ThreadNode>,
    notes: Notes,
    op_fid: u64,
    listed: Listed,
    #[prop(optional)] mut rest: Vec<ThreadNode>,
    #[prop(optional)] drawn: Option<RwSignal<bool>>,
) -> AnyView {
    if drawn.is_none() {
        nodes.append(&mut rest);
    }
    if !nodes.is_empty() {
        let later = drawn.map(|drawn| {
            let (notes, listed) = (notes.clone(), listed.clone());
            move || {
                drawn.get().then(
                    || view! { <CommentTree nodes=rest.clone() notes=notes.clone() op_fid listed=listed.clone()/> },
                )
            }
        });
        return view! {
            <div class="comments">
                {my_replies(&parent, listed.clone(), notes.clone(), op_fid)}
                <CommentTree nodes notes op_fid listed/>
                {later}
            </div>
        }
        .into_any();
    }
    #[cfg(not(feature = "static-host"))]
    if let Some(writer) = crate::components::writer::use_writer() {
        let (to, seen) = (parent.to_ascii_lowercase(), listed.clone());
        let any = Memo::new(move |_| !mine(writer, &to, &seen).is_empty());
        return (move || {
            if any.get() {
                view! { <div class="comments">{my_replies(&parent, listed.clone(), notes.clone(), op_fid)}</div> }
                    .into_any()
            } else {
                view! { <p class="empty">"No replies yet."</p> }.into_any()
            }
        })
        .into_any();
    }
    let _ = (parent, notes, op_fid, listed);
    view! { <p class="empty">"No replies yet."</p> }.into_any()
}

/// The casts a reply answers, root first, as full cards joined by a line
/// from each avatar to the next, and the reply below them (drawn by its
/// page). A chain too long for its room folds its middle behind "Show N
/// earlier", which opens in place; `earlier` says casts above the chain
/// couldn't be read.
#[component]
pub fn Ancestors(ancestors: Vec<Cast>, earlier: bool, notes: Notes) -> impl IntoView {
    let open = RwSignal::new(false);
    let (head, folded, tail) = match collapsed_gap(ancestors.len()) {
        Some(gap) => (ancestors[..gap.start].to_vec(), ancestors[gap.clone()].to_vec(), ancestors[gap.end..].to_vec()),
        None => (ancestors, Vec::new(), Vec::new()),
    };
    let folded_count = folded.len();
    let (folded_notes, tail_notes) = (notes.clone(), notes.clone());
    view! {
        <div class="thread-chain" role="group" aria-label="The casts this replies to">
            {earlier.then(|| view! {
                <p class="chain-note">"Earlier casts in this thread aren't loaded."</p>
                <div class="chain-link" aria-hidden="true"></div>
            })}
            {head.into_iter().map(|cast| chain_card(cast, notes.clone())).collect_view()}
            {(folded_count > 0).then(move || {
                let folded = folded.clone();
                let notes = folded_notes.clone();
                view! {
                    {move || (!open.get()).then(|| view! {
                        <button type="button" class="chain-more" aria-expanded="false" on:click=move |_| open.set(true)>
                            {format!("Show {folded_count} earlier")}
                        </button>
                        <div class="chain-link" aria-hidden="true"></div>
                    })}
                    {move || open.get().then(|| folded.clone().into_iter().map(|cast| chain_card(cast, notes.clone())).collect_view())}
                }
            })}
            {tail.into_iter().map(|cast| chain_card(cast, tail_notes.clone())).collect_view()}
        </div>
    }
}

/// One card of a chain, and the line down to the next thing in it.
fn chain_card(cast: Cast, notes: Notes) -> impl IntoView {
    view! {
        <div class="chain-item"><CastCard cast notes/></div>
        <div class="chain-link" aria-hidden="true"></div>
    }
}

#[component]
pub fn CommentTree(nodes: Vec<ThreadNode>, notes: Notes, op_fid: u64, listed: Listed) -> AnyView {
    nodes
        .into_iter()
        .map(|node| view! { <Comment node notes=notes.clone() op_fid listed=listed.clone()/> })
        .collect_view()
        .into_any()
}

/// How long a reply's body animates when it collapses or expands (its CSS
/// transition takes 340 ms).
const TOGGLE_MS: u64 = 400;

/// A reply, with its own replies. `mine`: one of the reader's from here
/// (`my_replies`), which, while it's on its way, says so in its time's
/// place (`posting`), and whose actions do nothing yet.
#[component]
fn Comment(
    node: ThreadNode,
    notes: Notes,
    op_fid: u64,
    listed: Listed,
    #[prop(optional)] mine: bool,
    #[prop(optional_no_strip)] posting: Option<&'static str>,
) -> AnyView {
    let guarded = StoredValue::new(node.cast.clone());
    let quotes = StoredValue::new(notes.quotes.clone());
    let props = StoredValue::new((node, notes, listed));
    view! { <Show when=move || !guarded.with_value(|cast| quotes.with_value(|quotes| crate::moderation::hides_cast(cast,quotes)))>
        {move || {let (node,notes,listed)=props.get_value();view! { <VisibleComment node notes op_fid listed mine posting/> } }}
    </Show> }.into_any()
}

#[component]
fn VisibleComment(
    node: ThreadNode,
    notes: Notes,
    op_fid: u64,
    listed: Listed,
    #[prop(optional)] mine: bool,
    #[prop(optional_no_strip)] posting: Option<&'static str>,
) -> AnyView {
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
    let mut cast = node.cast.clone();
    cast.references = allthing_core::moderation::References::with_quotes(&cast, &notes.quotes);
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
    #[cfg(not(feature = "static-host"))]
    let children = view! {
        <div class="children">
            {my_replies(&cast.hash, listed.clone(), notes.clone(), op_fid)}
            <CommentTree nodes=node.children.clone() notes=notes.clone() op_fid listed=listed.clone()/>
        </div>
    }
    .into_any();
    #[cfg(feature = "static-host")]
    let children = (!node.children.is_empty())
        .then(|| {
            view! { <div class="children"><CommentTree nodes=node.children.clone() notes=notes.clone() op_fid listed=listed.clone()/></div> }
        })
        .into_any();
    view! {
        <div
            class="comment"
            class:mine=mine
            class:pending=posting.is_some()
            class:collapsed=move || collapsed.get()
            class:animating=move || animating.get()
            class:deleted=gone
            aria-busy=posting.map(|_| "true")
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
                    {match posting {
                        Some(state) => view! { <span class="when posting">{state}</span> }.into_any(),
                        None => view! { <a class="when" href=href.clone()><span class="visually-hidden">"Open reply, "</span><TimeAgo ts=cast.ts/></a> }.into_any(),
                    }}
                </span>
                <OriginChip hash=cast.hash.clone() notes=notes.clone()/>
                <Show when=move || collapsed.get()>
                    <span class="hidden-count">{format!("{} hidden", plural(hidden as u64 + 1, "reply", "replies"))}</span>
                </Show>
            </div>
            <div class="comment-body" prop:inert=move || collapsed.get()>
                <div class="comment-inner">
                    <p class="deleted-note">"You deleted this reply."</p>
                    {if crate::miniapps::hides(&cast) {
                        let note = if cast.spam { "Post hidden" } else { "Mini app post hidden" };
                        view! { <p class="mini-app-note">{note}</p> }.into_any()
                    } else {
                        view! { <div class="comment-text"><CastText text=cast.text.clone() mentions=cast.mentions.clone()/></div> }.into_any()
                    }}
                    <MediaGrid cast=cast.clone()/>
                    <VideoLinks cast=cast.clone()/>
                    <LinkCards cast=cast.clone()/>
                    <crate::miniapps::MiniAppLink cast=cast.clone()/>
                    <div class="comment-foot" prop:inert=posting.is_some()>
                        <Engagement cast=cast.clone() tight=true/>
                        {reply_button(&cast, "reply")}
                    </div>
                    {posting.is_none().then(|| view! {
                        <ReactionChips cast=cast.clone() reacted=reactions(node.children.iter().map(|n| &n.cast))/>
                    })}
                    {children}
                    {(more > 0).then(|| view! {
                        <a class="continue" href=href>{format!("continue this thread ({}) →", plural(more, "more reply", "more replies"))}</a>
                    })}
                </div>
            </div>
        </div>
    }
    .into_any()
}

/// The reader's replies from here to `parent` that the page doesn't list
/// itself (`listed`) and that aren't deleted.
#[cfg(not(feature = "static-host"))]
fn mine(
    writer: crate::components::writer::Writer,
    parent: &str,
    listed: &HashSet<String>,
) -> Vec<crate::components::writer::Outgoing> {
    writer
        .replies_to(parent)
        .into_iter()
        .filter(|reply| reply.hash.as_ref().is_none_or(|hash| !listed.contains(hash) && !writer.deleted(hash)))
        .collect()
}

/// The reader's replies to `parent` from here, at the top of its replies,
/// each drawn as the thread draws a reply, until the page lists it itself
/// (`listed`): on its way, as posting, with its actions doing nothing yet;
/// then, once the node shows it, as a reply like the others. A reply handed
/// to the Farcaster app shows the same way, as posting there until it is.
#[cfg(not(feature = "static-host"))]
fn my_replies(parent: &str, listed: Listed, notes: Notes, op_fid: u64) -> AnyView {
    let Some(writer) = crate::components::writer::use_writer() else { return ().into_any() };
    let session = crate::components::signin::use_session();
    let parent = parent.to_ascii_lowercase();
    let shown = {
        let (parent, listed) = (parent.clone(), listed.clone());
        Memo::new(move |_| mine(writer, &parent, &listed))
    };
    // Drawn again, each, as one changes: there are one or two at most.
    (move || {
        let author = session.user.get()?;
        let replies = shown.get();
        (!replies.is_empty()).then(|| {
            replies
                .into_iter()
                .map(|reply| {
                    let posting = match (&reply.hash, reply.pending) {
                        (_, true) => Some("Posting…"),
                        (None, false) => Some("Posting in Farcaster"),
                        (Some(_), false) => None,
                    };
                    let node = ThreadNode { cast: reply_cast(&reply, author.clone(), &parent), children: Vec::new() };
                    view! { <Comment node notes=notes.clone() op_fid listed=listed.clone() mine=true posting/> }
                })
                .collect_view()
        })
    })
    .into_any()
}

#[cfg(feature = "static-host")]
fn my_replies(_: &str, _: Listed, _: Notes, _: u64) -> AnyView {
    ().into_any()
}

/// One of the reader's replies from here as the thread draws a cast: theirs,
/// from when it went, with its images, videos and GIF.
#[cfg(not(feature = "static-host"))]
fn reply_cast(
    reply: &crate::components::writer::Outgoing,
    author: allthing_core::model::UserSummary,
    parent: &str,
) -> allthing_core::model::Cast {
    use allthing_core::embeds::{MediaKind, classify_url};
    use allthing_core::model::Embed;
    let embeds = reply
        .media
        .iter()
        .map(|url| match classify_url(url, None) {
            MediaKind::Image => Embed::Image { url: url.clone() },
            MediaKind::Video => Embed::Video { url: url.clone() },
            MediaKind::Link => Embed::Link { url: url.clone(), image_url: None, title: None },
        })
        .collect();
    allthing_core::model::Cast {
        // Until it's signed, a stand-in in a hash's shape, for its ids.
        hash: reply.hash.clone().unwrap_or_else(|| format!("0x{:08x}{:032x}", reply.id as u32, reply.id)),
        author,
        text: reply.text.clone(),
        mentions: reply.mentions.clone(),
        ts: if reply.at > 0 { reply.at } else { crate::util::now() },
        parent_hash: Some(parent.to_owned()),
        parent_url: None,
        channel: None,
        embeds,
        likes: 0,
        recasts: 0,
        replies: 0,
        mini_app: false,
        references: reply.account_references(),
        spam: false,
    }
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

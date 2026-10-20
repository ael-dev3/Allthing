// SPDX-License-Identifier: AGPL-3.0-or-later
//! Emoji reactions to casts. Farcaster's own reactions are only likes and
//! recasts, so an emoji reaction is a reply that is one Pepe emoji and
//! nothing else (`allthing_core::emoji::reaction`): every Farcaster app
//! shows it as a reply, `:pepe_name:`, and Allthing gathers a cast's under
//! it as chips, with who reacted.

use allthing_core::emoji::{Emoji, Reacted};
use allthing_core::model::{Cast, UserSummary};
use leptos::prelude::*;

use crate::util::{href, prefers_reduced_motion};

/// "alice, bob and 3 others".
fn names(by: &[UserSummary]) -> String {
    let shown: Vec<String> = by.iter().take(3).map(|u| format!("@{}", u.username)).collect();
    match by.len().saturating_sub(shown.len()) {
        0 => shown.join(", "),
        1 => format!("{} and 1 other", shown.join(", ")),
        n => format!("{} and {n} others", shown.join(", ")),
    }
}

/// One chip: its emoji, how many reacted with it, whether the reader did,
/// who (for its label), and the reader's replies that are it.
struct Chip {
    emoji: &'static Emoji,
    count: u64,
    own: bool,
    who: String,
    /// Deleted to take the reader's reaction back.
    replies: Vec<String>,
}

/// A cast's emoji reactions, from its replies (`reacted`), with the
/// reader's own this visit. A chip reacts with its emoji too, and the
/// reader's own takes their reaction back: the reply that is it is deleted.
#[component]
pub fn ReactionChips(cast: Cast, reacted: Vec<Reacted>) -> impl IntoView {
    // The static copy shows them, and has no one to react as.
    #[cfg(feature = "static-host")]
    let _ = cast;
    #[cfg(not(feature = "static-host"))]
    let hash = cast.hash.clone();
    #[cfg(not(feature = "static-host"))]
    let writer = crate::components::writer::use_writer();
    #[cfg(not(feature = "static-host"))]
    let target = allthing_core::message::CastId::new(cast.author.fid, &cast.hash);
    #[cfg(not(feature = "static-host"))]
    let session = crate::components::signin::use_session();
    let motion = !prefers_reduced_motion();
    move || {
        // Replies the reader deleted this visit.
        #[cfg(not(feature = "static-host"))]
        let gone = |reply: &str| writer.is_some_and(|w| w.deleted(reply));
        #[cfg(feature = "static-host")]
        let gone = |_: &str| false;
        // The reader's own this visit, as reactions the replies don't have
        // yet, with the reply each is once it's known; less those taken back.
        #[cfg(not(feature = "static-host"))]
        let mine: Vec<(&'static Emoji, Option<String>)> = writer
            .map(|w| w.reactions(&hash).into_iter().map(|e| (e, w.reaction_reply(&hash, e))).collect())
            .unwrap_or_default();
        #[cfg(feature = "static-host")]
        let mine: Vec<(&'static Emoji, Option<String>)> = Vec::new();
        let mine: Vec<(&'static Emoji, Option<String>)> =
            mine.into_iter().filter(|(_, reply)| !reply.as_deref().is_some_and(gone)).collect();
        let reader = {
            #[cfg(not(feature = "static-host"))]
            {
                writer.and_then(|w| w.reader())
            }
            #[cfg(feature = "static-host")]
            {
                None::<u64>
            }
        };
        let mut chips: Vec<Chip> = reacted
            .iter()
            .filter_map(|r| {
                // An account whose replies with it are all deleted no longer
                // reacts with it.
                let by: Vec<UserSummary> =
                    r.by.iter()
                        .filter(|u| r.replies.iter().any(|(fid, reply)| *fid == u.fid && !gone(reply)))
                        .cloned()
                        .collect();
                let mut replies: Vec<String> =
                    reader.map(|fid| r.replies_of(fid)).unwrap_or_default().into_iter().filter(|h| !gone(h)).collect();
                let by_reader = !replies.is_empty();
                let this_visit = mine.iter().find(|(e, _)| *e == r.emoji);
                let added = !by_reader && this_visit.is_some();
                if let Some((_, Some(reply))) = this_visit
                    && !replies.contains(reply)
                {
                    replies.push(reply.clone());
                }
                let count = by.len() as u64 + u64::from(added);
                let who = match (added, by.is_empty()) {
                    (true, true) => "you".to_owned(),
                    (true, false) => format!("you, {}", names(&by)),
                    (false, _) => names(&by),
                };
                (count > 0).then_some(Chip { emoji: r.emoji, count, own: by_reader || added, who, replies })
            })
            .collect();
        for (emoji, reply) in mine.iter() {
            if !chips.iter().any(|c| c.emoji == *emoji) {
                chips.push(Chip {
                    emoji,
                    count: 1,
                    own: true,
                    who: "you".into(),
                    replies: reply.iter().cloned().collect(),
                });
            }
        }
        if chips.is_empty() {
            return ().into_any();
        }
        view! {
            <ul class="reaction-chips" aria-label="Emoji reactions">
                {chips.into_iter().map(|Chip { emoji, count, own, who, replies }| {
                    let src = href(&if motion { emoji.file() } else { emoji.still() });
                    // The reader's, handed to the Farcaster app and not posted
                    // yet, or being taken back.
                    #[cfg(not(feature = "static-host"))]
                    let pending = own
                        && writer.is_some_and(|w| w.pending_emoji(&hash, emoji) || replies.iter().any(|r| w.deleting(r)));
                    #[cfg(feature = "static-host")]
                    let pending = false;
                    let take_back = own && !replies.is_empty();
                    let label = format!(
                        "{}: {count} {}, by {who}{}",
                        emoji.label(),
                        if count == 1 { "reaction" } else { "reactions" },
                        if take_back {
                            ". Take yours back"
                        } else if own {
                            ""
                        } else {
                            ". React with it too"
                        },
                    );
                    #[cfg(not(feature = "static-host"))]
                    let act = {
                        let hash = hash.clone();
                        move |_| {
                            let (Some(writer), Some(target)) = (writer, target) else { return };
                            if !own {
                                writer.react(target, emoji, react_ask(&hash, emoji));
                            } else if let Some(reply) = replies.first() {
                                let username = session.user.with_untracked(|u| u.as_ref().map(|u| u.username.clone()));
                                writer.delete(reply.clone(), take_back_ask(&username.unwrap_or_default(), reply));
                            }
                        }
                    };
                    #[cfg(feature = "static-host")]
                    let act = move |_| {
                        let _ = &replies;
                    };
                    view! {
                        <li>
                            <button
                                type="button"
                                class="reaction-chip"
                                class:pending=pending
                                aria-pressed=own.to_string()
                                aria-label=label
                                on:click=act
                            >
                                <img src=src alt="" width=emoji.width / 2 height=emoji.height / 2 loading="lazy"/>
                                <span class="n">{count}</span>
                            </button>
                        </li>
                    }
                }).collect_view()}
            </ul>
        }
        .into_any()
    }
}

/// Taking a Pepe reaction back in the Farcaster app: the reader's reply
/// that is it, to delete there.
#[cfg(not(feature = "static-host"))]
fn take_back_ask(username: &str, reply: &str) -> crate::components::handoff::Ask {
    crate::components::handoff::Ask::delete(allthing_core::compose::cast_url(username, reply), "reaction")
}

/// A Pepe reaction handed to the Farcaster app: the reply that is the
/// emoji, ready to post there.
#[cfg(not(feature = "static-host"))]
fn react_ask(hash: &str, emoji: &Emoji) -> crate::components::handoff::Ask {
    let reply = allthing_core::compose::Draft {
        text: format!(":{}:", emoji.code),
        parent_hash: Some(hash.to_owned()),
        ..Default::default()
    };
    crate::components::handoff::Ask::react(reply.compose_url(), emoji.code)
}

/// React, under a post or a reply: the Pepe picker, whose pick goes out as
/// a reply, from here or, where Allthing doesn't post for the reader, from
/// the Farcaster app.
#[cfg(not(feature = "static-host"))]
#[component]
pub fn ReactButton(cast: Cast) -> impl IntoView {
    use crate::components::emoji_picker::{self, Pick, Tab};
    use crate::components::popover::Popover;
    use crate::components::writer::use_writer;

    let (Some(writer), Some(target)) = (use_writer(), allthing_core::message::CastId::new(cast.author.fid, &cast.hash))
    else {
        return ().into_any();
    };
    let hash = cast.hash.clone();
    let shut = RwSignal::new(0u32);
    let tab = RwSignal::new(Tab::Emoji);
    let picked = {
        let hash = hash.clone();
        Callback::new(move |(emoji, _): Pick| {
            emoji_picker::used(emoji);
            shut.update(|n| *n += 1);
            writer.react(target, emoji, react_ask(&hash, emoji));
        })
    };
    let label = format!("React to @{}'s cast with an emoji", cast.author.username);
    (move || {
        let hash = hash.clone();
        let label = label.clone();
        {
            view! {
                <Popover
                    label="React with a Pepe"
                    trigger_label=label
                    trigger_class="reply-button react-button"
                    class="react"
                    panel_class="react-panel"
                    max_px=420.0
                    shut=shut
                    trigger=move || {
                        let hash = hash.clone();
                        view! {
                            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=SMILE/></svg>
                            <span class="react-label">{move || if writer.reacting(&hash) { "Reacting…" } else { "React" }}</span>
                        }
                    }
                    content=move || crate::split::lazy_view(|| view! { <p class="muted react-pending">"Loading…"</p> }, react_picker(tab, picked))
                    on_warm=Callback::new(|()| {
                        leptos::task::spawn_local(async {
                            let _ = preload_react_picker().await;
                        });
                    })
                />
            }
        }
    })
    .into_any()
}

/// React's Pepe picker, whose code (the composer's emoji picker) is fetched
/// the first time it opens, or as React is pointed at or focused
/// (`crate::split`).
#[cfg(not(feature = "static-host"))]
#[leptos::wasm_split::wasm_split(react, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_react_picker))]
fn react_picker(
    tab: RwSignal<crate::components::emoji_picker::Tab>,
    picked: Callback<crate::components::emoji_picker::Pick>,
) -> Result<AnyView, leptos::wasm_split::SplitLoaderError> {
    use crate::components::emoji_picker::EmojiPicker;
    Ok(view! { <EmojiPicker tab picked/> }.into_any())
}

#[cfg(not(feature = "static-host"))]
const SMILE: &str = "M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12zM5.6 9.4a3 3 0 0 0 4.8 0M6 6.4v.2M10 6.4v.2";

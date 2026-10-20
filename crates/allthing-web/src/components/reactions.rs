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

/// A cast's emoji reactions, from its replies (`reacted`), with the
/// reader's own this visit. A chip reacts with its emoji too.
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
    let motion = !prefers_reduced_motion();
    move || {
        // The reader's own this visit, as reactions the replies don't have yet.
        #[cfg(not(feature = "static-host"))]
        let mine: Vec<&'static Emoji> = writer.map(|w| w.reactions(&hash)).unwrap_or_default();
        #[cfg(feature = "static-host")]
        let mine: Vec<&'static Emoji> = Vec::new();
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
        let mut chips: Vec<(&'static Emoji, u64, bool, String)> = reacted
            .iter()
            .map(|r| {
                let by_reader = reader.is_some_and(|fid| r.by.iter().any(|u| u.fid == fid));
                let added = !by_reader && mine.contains(&r.emoji);
                let count = r.by.len() as u64 + u64::from(added);
                let who = if added { format!("you, {}", names(&r.by)) } else { names(&r.by) };
                (r.emoji, count, by_reader || added, who)
            })
            .collect();
        for emoji in mine.iter().filter(|e| !reacted.iter().any(|r| r.emoji == **e)) {
            chips.push((emoji, 1, true, "you".into()));
        }
        if chips.is_empty() {
            return ().into_any();
        }
        view! {
            <ul class="reaction-chips" aria-label="Emoji reactions">
                {chips.into_iter().map(|(emoji, count, own, who)| {
                    let src = href(&if motion { emoji.file() } else { emoji.still() });
                    // The reader's, handed to the Farcaster app and not posted yet.
                    #[cfg(not(feature = "static-host"))]
                    let pending = own && writer.is_some_and(|w| w.pending_emoji(&hash, emoji));
                    #[cfg(feature = "static-host")]
                    let pending = false;
                    let label = format!(
                        "{}: {count} {}, by {who}{}",
                        emoji.label(),
                        if count == 1 { "reaction" } else { "reactions" },
                        if own { "" } else { ". React with it too" },
                    );
                    #[cfg(not(feature = "static-host"))]
                    let react = {
                        let hash = hash.clone();
                        move |_| {
                            if let (Some(writer), Some(target)) = (writer, target)
                                && !own
                            {
                                writer.react(target, emoji, react_ask(&hash, emoji));
                            }
                        }
                    };
                    #[cfg(feature = "static-host")]
                    let react = move |_| {};
                    view! {
                        <li>
                            <button
                                type="button"
                                class="reaction-chip"
                                class:pending=pending
                                aria-pressed=own.to_string()
                                aria-label=label
                                on:click=react
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
    use crate::components::emoji_picker::{self, EmojiPicker, Pick, Tab};
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
                    content=move || view! { <EmojiPicker tab picked/> }
                />
            }
        }
    })
    .into_any()
}

#[cfg(not(feature = "static-host"))]
const SMILE: &str = "M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12zM5.6 9.4a3 3 0 0 0 4.8 0M6 6.4v.2M10 6.4v.2";

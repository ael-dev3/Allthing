// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reusable view pieces.

#[cfg(not(feature = "static-host"))]
pub mod attach;
pub mod cast;
pub mod chips;
pub mod clients;
#[cfg(not(feature = "static-host"))]
pub mod compose;
#[cfg(not(feature = "static-host"))]
pub mod dc_key;
#[cfg(not(feature = "static-host"))]
pub mod editor;
#[cfg(not(feature = "static-host"))]
pub mod emoji_picker;
pub mod facts;
pub mod favorite;
#[cfg(not(feature = "static-host"))]
pub mod handoff;
pub mod layout;
pub mod media;
#[cfg(not(feature = "static-host"))]
pub mod notifications;
#[cfg(not(feature = "static-host"))]
pub mod outgoing;
pub mod player;
pub mod popover;
pub mod profile_card;
pub mod quote;
pub mod reactions;
pub mod search;
pub mod show_less;
#[cfg(not(feature = "static-host"))]
pub mod signin;
pub mod text;
pub mod thread;
pub mod who;
#[cfg(not(feature = "static-host"))]
pub mod writer;

/// Whether the reader deleted the cast `hash` this visit (never, in the
/// static copy, which has no sign-in).
pub fn deleted(hash: &str) -> leptos::prelude::Signal<bool> {
    #[cfg(not(feature = "static-host"))]
    {
        writer::deleted_memo(hash.to_owned()).into()
    }
    #[cfg(feature = "static-host")]
    {
        let _ = hash;
        leptos::prelude::Signal::stored(false)
    }
}

/// The reader's replies, handed to the Farcaster app or posted from here,
/// to the cast `hashes` (a post's, and its replies'), above its replies,
/// until the thread has them; none in the static copy, which has no
/// sign-in.
pub fn outgoing_replies(hashes: std::collections::HashSet<String>) -> leptos::prelude::AnyView {
    use leptos::prelude::*;
    #[cfg(not(feature = "static-host"))]
    {
        let listed = Signal::stored(hashes.clone());
        view! { <outgoing::OutgoingCasts place=outgoing::Place::Replies(hashes) listed/> }.into_any()
    }
    #[cfg(feature = "static-host")]
    {
        let _ = hashes;
        ().into_any()
    }
}

/// The reader's casts and quotes, handed to the Farcaster app or posted
/// from here, above a feed: a channel's (`channel`), its own; until the
/// feed lists them (`listed`). None in the static copy.
pub fn outgoing_feed(
    channel: Option<String>,
    listed: leptos::prelude::Signal<std::collections::HashSet<String>>,
) -> leptos::prelude::AnyView {
    use leptos::prelude::*;
    #[cfg(not(feature = "static-host"))]
    {
        view! { <outgoing::OutgoingCasts place=outgoing::Place::Feed(channel) listed/> }.into_any()
    }
    #[cfg(feature = "static-host")]
    {
        let _ = (channel, listed);
        ().into_any()
    }
}

/// On the reader's own profile (`fid`'s), their casts and quotes, or on
/// its Replies tab (`replies`) their replies, above its list until it
/// lists them (`listed`). None in the static copy.
pub fn outgoing_profile(
    fid: u64,
    replies: leptos::prelude::Signal<bool>,
    listed: leptos::prelude::Signal<std::collections::HashSet<String>>,
) -> leptos::prelude::AnyView {
    use leptos::prelude::*;
    #[cfg(not(feature = "static-host"))]
    {
        view! { <outgoing::OutgoingCasts place=outgoing::Place::Profile { fid, replies } listed/> }.into_any()
    }
    #[cfg(feature = "static-host")]
    {
        let _ = (fid, replies, listed);
        ().into_any()
    }
}

/// The hashes of the casts a list shows, for what's above it to leave out.
pub fn listed_hashes(
    casts: leptos::prelude::Signal<Vec<allthing_core::model::Cast>>,
) -> leptos::prelude::Signal<std::collections::HashSet<String>> {
    use leptos::prelude::*;
    Signal::derive(move || casts.with(|casts| casts.iter().map(|cast| cast.hash.to_ascii_lowercase()).collect()))
}

/// Under a post, for a reader signed in: "Cast your reply", which opens
/// Reply (`compose::ReplyPrompt`); none in the static copy, which has no
/// sign-in.
pub fn reply_prompt(cast: &allthing_core::model::Cast) -> leptos::prelude::AnyView {
    use leptos::prelude::*;
    #[cfg(not(feature = "static-host"))]
    {
        view! { <compose::ReplyPrompt cast=cast.clone()/> }.into_any()
    }
    #[cfg(feature = "static-host")]
    {
        let _ = cast;
        ().into_any()
    }
}

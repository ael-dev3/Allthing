// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reusable view pieces.

pub mod cast;
pub mod chips;
pub mod clients;
#[cfg(not(feature = "static-host"))]
pub mod compose;
#[cfg(not(feature = "static-host"))]
pub mod emoji_picker;
pub mod facts;
pub mod favorite;
#[cfg(not(feature = "static-host"))]
pub mod handoff;
pub mod layout;
pub mod media;
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

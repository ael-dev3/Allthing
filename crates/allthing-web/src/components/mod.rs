// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reusable view pieces.

pub mod cast;
pub mod chips;
pub mod clients;
#[cfg(not(feature = "static-host"))]
pub mod compose;
pub mod facts;
pub mod layout;
pub mod media;
pub mod player;
pub mod popover;
pub mod profile_card;
pub mod quote;
pub mod search;
#[cfg(not(feature = "static-host"))]
pub mod signin;
pub mod text;
pub mod thread;
pub mod who;

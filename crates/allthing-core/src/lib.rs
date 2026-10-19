// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared, IO-free logic for Allthing: the normalized Farcaster model, the
//! Neynar-shaped JSON it is normalized from, ranking, text and media
//! handling, provenance decoding, wallet shares, the content spam filter,
//! profile cards, and the budget primitives the Worker uses to stay inside
//! Cloudflare's limits.
//!
//! Everything here compiles both for the browser app and for the Worker, so it
//! avoids heavy dependencies (no regex, URL or date crates).

pub mod api;
pub mod channels;
pub mod clients;
pub mod compose;
pub mod embeds;
pub mod format;
pub mod guard;
pub mod hls;
pub mod index;
pub mod latest;
pub mod listing;
pub mod model;
pub mod normalize;
pub mod params;
pub mod profile;
pub mod provenance;
pub mod quotes;
pub mod ranking;
pub mod siwf;
pub mod spam;
pub mod text;
pub mod thread;
pub mod time;
pub mod video;
pub mod vote;
pub mod wallet;

pub use model::*;

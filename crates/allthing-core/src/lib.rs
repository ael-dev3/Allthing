// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared, IO-free logic for Allthing: the normalized Farcaster model, the
//! Neynar-shaped JSON it is normalized from, ranking, text and media
//! handling, provenance decoding, wallet shares, the content spam filter,
//! profile cards and websites, the rules of readers' attachments, and the
//! budget primitives the Worker uses to stay inside Cloudflare's limits.
//!
//! Everything here compiles both for the browser app and for the Worker, so it
//! avoids heavy dependencies (no regex, URL or date crates).

pub mod activity;
pub mod api;
pub mod app_key;
pub mod channels;
pub mod clients;
pub mod compose;
pub mod drafts;
pub mod e2e;
pub mod embeds;
pub mod emoji;
pub mod fmp4;
pub mod following;
pub mod follows;
pub mod format;
pub mod guard;
pub mod hls;
pub mod index;
pub mod latest;
pub mod library;
pub mod listing;
pub mod media;
pub mod message;
pub mod metadata;
pub mod model;
pub mod moderation;
pub mod normalize;
pub mod notification_state;
pub mod notifications;
pub mod params;
pub mod people;
pub mod poll;
pub mod presence;
pub mod profile;
pub mod provenance;
pub mod quotes;
pub mod ranking;
pub mod scheduled;
pub mod seen;
pub mod siwf;
pub mod spam;
pub mod stdb;
pub mod text;
pub mod thread;
pub mod time;
pub mod video;
pub mod vote;
pub mod wallet;
pub mod website;

pub use model::*;

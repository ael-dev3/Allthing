// SPDX-License-Identifier: AGPL-3.0-or-later
//! Whom the signed-in reader follows, as far as the front page's **For you**
//! needs to know: the authors of the casts on the first page of their
//! Following feed (`allthing_core::following::followed_authors`), the
//! accounts they follow that cast lately. For you lifts their casts
//! (`allthing_core::ranking::FOLLOWED_HOT_WEIGHT`).
//!
//! No request is made for it while a page is drawn: For you ranks by the set
//! kept here, in this browser's localStorage (`allthing.followed`, with the
//! reader's fid, so another account's is never used), and sent nowhere.
//! The set is renewed whenever the Following tab's first page comes, and,
//! once it is older than [`STALE_MS`], by one background read of that page
//! after For you is drawn ([`refresh`], through `api::prefetch`), which also
//! makes the Following tab instant; that read is tried at most once per
//! [`STALE_MS`], and while it fails For you ranks by the set last kept, or
//! without the lift. The list on screen isn't re-sorted when
//! it lands; the next one drawn uses it (docs/BUDGET.md). `web/paint.js`
//! reads the same key.

use std::collections::BTreeSet;

use allthing_core::following::{FOLLOWING_PATH, FollowingPayload, followed_authors};
use serde::{Deserialize, Serialize};

const KEY: &str = "allthing.followed";
/// When [`refresh`] last asked for the set (ms since the epoch), so a read
/// that fails isn't tried again on every page before [`STALE_MS`] passes.
const TRIED_KEY: &str = "allthing.followed-tried";

/// A set older than this is renewed in the background.
pub const STALE_MS: f64 = 10.0 * 60.0 * 1000.0;

#[derive(Default, Serialize, Deserialize)]
struct Kept {
    reader: u64,
    /// When it was read (ms since the epoch).
    at: f64,
    fids: BTreeSet<u64>,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn read() -> Option<Kept> {
    let text = storage()?.get_item(KEY).ok().flatten()?;
    serde_json::from_str(&text).ok()
}

/// The accounts `reader` follows, as last kept; none for another reader.
pub fn followed(reader: u64) -> BTreeSet<u64> {
    read().filter(|kept| kept.reader == reader).map(|kept| kept.fids).unwrap_or_default()
}

/// Keep the authors of `reader`'s first Following page.
pub fn remember(reader: u64, first_page: &FollowingPayload) {
    let kept = Kept { reader, at: js_sys::Date::now(), fids: followed_authors(first_page) };
    if let (Some(storage), Ok(text)) = (storage(), serde_json::to_string(&kept)) {
        let _ = storage.set_item(KEY, &text);
    }
}

/// How many times [`refresh`] waits for a place among the prefetches (the
/// page's own fill them as it loads), [`RETRY_MS`] apart, before giving up
/// until the next page.
const RETRIES: u32 = 5;
const RETRY_MS: u64 = 2_000;

/// Renew `reader`'s set in the background if it's missing or stale.
pub fn refresh(reader: u64) {
    refresh_trying(reader, RETRIES);
}

fn refresh_trying(reader: u64, left: u32) {
    let fresh = read().is_some_and(|kept| kept.reader == reader && js_sys::Date::now() - kept.at < STALE_MS);
    let tried = storage()
        .and_then(|s| s.get_item(TRIED_KEY).ok().flatten())
        .and_then(|t| t.parse::<f64>().ok())
        .is_some_and(|at| js_sys::Date::now() - at < STALE_MS);
    if fresh || tried {
        return;
    }
    let started = crate::api::prefetch_then(FOLLOWING_PATH, move |body| {
        if let Ok(page) = serde_json::from_str::<FollowingPayload>(body) {
            remember(reader, &page);
        }
    });
    if started != crate::api::Prefetch::Full
        && let Some(storage) = storage()
    {
        let _ = storage.set_item(TRIED_KEY, &js_sys::Date::now().to_string());
    }
    if started == crate::api::Prefetch::Full && left > 0 {
        leptos::prelude::set_timeout(
            move || refresh_trying(reader, left - 1),
            std::time::Duration::from_millis(RETRY_MS),
        );
    }
}

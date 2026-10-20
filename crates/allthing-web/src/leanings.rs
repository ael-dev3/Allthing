// SPDX-License-Identifier: AGPL-3.0-or-later
//! The reader's leanings about accounts (`allthing_core::ranking::Leanings`):
//! their favorites, kept in this browser's localStorage (`allthing.favorites`)
//! and sent nowhere. A favorite's casts rise in the reader's hot and top
//! feeds, and carry a star there. The reader's other tabs follow a change
//! (the `storage` event). The static copy has them too.

use allthing_core::ranking::Leanings;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

const FAVORITES: &str = "allthing.favorites";

/// A favorite: the account, and its username when it was made one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favorite {
    pub fid: u64,
    pub username: String,
}

/// The reader's leanings, held by the app.
#[derive(Clone, Copy)]
pub struct Leaning {
    favorites: RwSignal<Vec<Favorite>>,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn load() -> Vec<Favorite> {
    let kept = storage().and_then(|s| s.get_item(FAVORITES).ok().flatten());
    kept.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

impl Leaning {
    pub fn new() -> Leaning {
        let leaning = Leaning { favorites: RwSignal::new(load()) };
        // For as long as the app runs: another tab changed them.
        let _ = window_event_listener(leptos::ev::storage, move |ev| {
            if ev.key().as_deref() == Some(FAVORITES) {
                leaning.favorites.set(load());
            }
        });
        leaning
    }

    /// The reader's leanings, for sorting a feed.
    pub fn leanings(self) -> Leanings {
        Leanings { favorites: self.favorites.with(|all| all.iter().map(|f| f.fid).collect()) }
    }

    pub fn is_favorite(self, fid: u64) -> bool {
        self.favorites.with(|all| all.iter().any(|f| f.fid == fid))
    }

    /// Make `fid` a favorite, or not any more.
    pub fn toggle_favorite(self, fid: u64, username: &str) {
        self.favorites.update(|all| match all.iter().position(|f| f.fid == fid) {
            Some(at) => {
                all.remove(at);
            }
            None => all.push(Favorite { fid, username: username.to_owned() }),
        });
        if let Some(storage) = storage() {
            let kept = self.favorites.with_untracked(|all| serde_json::to_string(all).unwrap_or_default());
            let _ = storage.set_item(FAVORITES, &kept);
        }
    }
}

impl Default for Leaning {
    fn default() -> Self {
        Leaning::new()
    }
}

pub fn use_leaning() -> Leaning {
    expect_context::<Leaning>()
}

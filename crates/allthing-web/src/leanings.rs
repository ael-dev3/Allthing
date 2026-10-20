// SPDX-License-Identifier: AGPL-3.0-or-later
//! The reader's leanings about accounts (`allthing_core::ranking::Leanings`):
//! their favorites and those they see less of, kept in this browser's
//! localStorage (`allthing.favorites`, `allthing.show-less`) and sent nowhere.
//! A favorite's casts rise in the reader's hot and top feeds, and carry a star
//! there; the casts of an account they see less of sit a little lower, with no
//! mark, and nothing is hidden. An account is one or the other, or neither.
//! The reader's other tabs follow a change (the `storage` event). The static
//! copy has them too.

use std::collections::BTreeSet;

use allthing_core::ranking::Leanings;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// An account the reader leans on: its fid, and its username when they did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub fid: u64,
    pub username: String,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// One list of accounts, kept under its localStorage key.
#[derive(Clone, Copy)]
struct Kept {
    accounts: RwSignal<Vec<Account>>,
    key: &'static str,
}

impl Kept {
    fn new(key: &'static str) -> Kept {
        Kept { accounts: RwSignal::new(Kept::read(key)), key }
    }

    fn read(key: &str) -> Vec<Account> {
        let kept = storage().and_then(|s| s.get_item(key).ok().flatten());
        kept.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    fn reload(self) {
        self.accounts.set(Kept::read(self.key));
    }

    fn save(self) {
        if let Some(storage) = storage() {
            let kept = self.accounts.with_untracked(|all| serde_json::to_string(all).unwrap_or_default());
            let _ = storage.set_item(self.key, &kept);
        }
    }

    fn has(self, fid: u64) -> bool {
        self.accounts.with(|all| all.iter().any(|a| a.fid == fid))
    }

    fn fids(self) -> BTreeSet<u64> {
        self.accounts.with(|all| all.iter().map(|a| a.fid).collect())
    }

    /// Take `fid` off the list, if it's there; whether it was.
    fn take_off(self, fid: u64) -> bool {
        let there = self.accounts.with_untracked(|all| all.iter().any(|a| a.fid == fid));
        if there {
            self.accounts.update(|all| all.retain(|a| a.fid != fid));
            self.save();
        }
        there
    }

    fn put_on(self, fid: u64, username: &str) {
        self.accounts.update(|all| all.push(Account { fid, username: username.to_owned() }));
        self.save();
    }
}

/// The reader's leanings, held by the app.
#[derive(Clone, Copy)]
pub struct Leaning {
    favorites: Kept,
    less: Kept,
}

impl Leaning {
    pub fn new() -> Leaning {
        let leaning = Leaning { favorites: Kept::new("allthing.favorites"), less: Kept::new("allthing.show-less") };
        // For as long as the app runs: another tab changed them.
        let _ = window_event_listener(leptos::ev::storage, move |ev| {
            for kept in [leaning.favorites, leaning.less] {
                if ev.key().as_deref() == Some(kept.key) {
                    kept.reload();
                }
            }
        });
        leaning
    }

    /// The reader's leanings, for sorting a feed.
    pub fn leanings(self) -> Leanings {
        Leanings { favorites: self.favorites.fids(), less: self.less.fids() }
    }

    pub fn is_favorite(self, fid: u64) -> bool {
        self.favorites.has(fid)
    }

    pub fn shows_less(self, fid: u64) -> bool {
        self.less.has(fid)
    }

    /// Make `fid` a favorite (and no longer one the reader sees less of), or
    /// not a favorite any more.
    pub fn toggle_favorite(self, fid: u64, username: &str) {
        toggle(self.favorites, self.less, fid, username);
    }

    /// See less of `fid` (and no longer as a favorite), or as much as of
    /// anyone again.
    pub fn toggle_less(self, fid: u64, username: &str) {
        toggle(self.less, self.favorites, fid, username);
    }
}

/// Put `fid` on `list`, and off `other`; or, if it was on `list`, off it.
fn toggle(list: Kept, other: Kept, fid: u64, username: &str) {
    if !list.take_off(fid) {
        other.take_off(fid);
        list.put_on(fid, username);
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

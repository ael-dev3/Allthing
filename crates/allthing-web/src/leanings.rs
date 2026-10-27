// SPDX-License-Identifier: AGPL-3.0-or-later
//! The reader's leanings about accounts (`allthing_core::ranking::Leanings`):
//! their favorites and those they see less of, kept in this browser's
//! localStorage (`allthing.favorites`, `allthing.show-less`) and sent nowhere.
//! A favorite's casts rise well up the reader's hot and top feeds, and carry
//! a star there; Show less omits about 90% of that account's feed posts and
//! lowers the rest in hot/top. New retains chronological order
//! (`allthing_core::ranking::sorted_for`). An account is one or the other,
//! or neither.
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
    /// How many times the reader's other tabs changed them while this one
    /// ran: a list on screen follows those (`for_list`).
    elsewhere: RwSignal<u32>,
}

impl Leaning {
    pub fn new() -> Leaning {
        let leaning = Leaning {
            favorites: Kept::new("allthing.favorites"),
            less: Kept::new("allthing.show-less"),
            elsewhere: RwSignal::new(0),
        };
        // For as long as the app runs: another tab changed them.
        let _ = window_event_listener(leptos::ev::storage, move |ev| {
            for kept in [leaning.favorites, leaning.less] {
                if ev.key().as_deref() == Some(kept.key) {
                    kept.reload();
                    leaning.elsewhere.update(|n| *n = n.wrapping_add(1));
                }
            }
        });
        leaning
    }

    /// The reader's leanings, for sorting a feed.
    pub fn leanings(self) -> Leanings {
        Leanings { favorites: self.favorites.fids(), less: self.less.fids(), ..Leanings::default() }
    }

    /// The leanings to sort a list on screen by, under `key` (its order, its
    /// filter): those it was last sorted by while the key is the same, and
    /// the reader's own when it changes, or when another tab changes them.
    /// So a favorite made, or Show less, in a profile card over the list
    /// leaves its cards where they are, under the pointer, until the list is
    /// next loaded, sorted or filtered; another tab's re-sorts it in place.
    pub fn for_list<K>(self, sorted_by: StoredValue<Option<(K, u32, Leanings)>>, key: K) -> Leanings
    where
        K: Clone + PartialEq + Send + Sync + 'static,
    {
        let elsewhere = self.elsewhere.get();
        match sorted_by.get_value() {
            Some((was, at, leanings)) if was == key && at == elsewhere => leanings,
            _ => {
                let now = untrack(|| self.leanings());
                sorted_by.set_value(Some((key, elsewhere, now.clone())));
                now
            }
        }
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

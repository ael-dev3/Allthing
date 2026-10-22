// SPDX-License-Identifier: AGPL-3.0-or-later
//! The people of a followers or following list as the page has them, and
//! what this visit loaded of each list: going back to a list shows every
//! person it had, at once and as tall, so Back puts the reader where they
//! were (`crate::nav`).

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

use allthing_core::api::{FollowsDir, FollowsOwner, FollowsPayload, Person};
use leptos::prelude::*;

use crate::api::Failure;

/// How long a list this visit loaded is shown again as it was: as long as
/// the app keeps a page it read (`crate::api`).
const KEPT_MS: f64 = 300_000.0;
/// Lists kept at once.
const KEPT_LISTS: usize = 4;

/// A list: its account, by the name its address gives, and which list.
pub type ListKey = (String, FollowsDir);

struct Kept {
    key: ListKey,
    at: f64,
    owner: FollowsOwner,
    people: Vec<Arc<Person>>,
    next: Option<String>,
}

thread_local! {
    static KEPT: RefCell<Vec<Kept>> = const { RefCell::new(Vec::new()) };
}

/// The account a list this visit loaded belongs to, either of its lists,
/// for the other's header before its own first page is in.
pub fn kept_owner(name: &str) -> Option<FollowsOwner> {
    let now = js_sys::Date::now();
    KEPT.with(|kept| {
        kept.borrow().iter().rev().find(|k| k.key.0 == name && now - k.at < KEPT_MS).map(|k| k.owner.clone())
    })
}

/// A list as the page has it.
#[derive(Clone, Copy)]
pub struct People {
    key: StoredValue<ListKey>,
    /// Its account: from its first page, or until then from what the visit
    /// knows of it (`preview`).
    pub owner: RwSignal<Option<FollowsOwner>>,
    /// Its people, the newest follow first.
    pub people: RwSignal<Vec<Arc<Person>>>,
    /// Where it goes on (`cursor=`), once its first page is in; `None` at its end.
    pub next: RwSignal<Option<String>>,
    /// Whether its first page is in.
    pub loaded: RwSignal<bool>,
    /// Why its first page couldn't be read.
    pub failed: RwSignal<Option<Failure>>,
    listed: StoredValue<HashSet<u64>>,
}

impl People {
    /// The list as this visit left it, if lately; otherwise none of it yet,
    /// with its account as far as `preview` knows it.
    pub fn new(key: ListKey, preview: Option<FollowsOwner>) -> People {
        let now = js_sys::Date::now();
        let kept = KEPT.with(|kept| {
            let kept = kept.borrow();
            kept.iter()
                .find(|k| k.key == key && now - k.at < KEPT_MS)
                .map(|k| (k.owner.clone(), k.people.clone(), k.next.clone()))
        });
        let loaded = kept.is_some();
        let (owner, people, next) = match kept {
            Some((owner, people, next)) => (Some(owner), people, next),
            None => (preview, Vec::new(), None),
        };
        let listed = people.iter().map(|p| p.fid).collect();
        People {
            key: StoredValue::new(key),
            owner: RwSignal::new(owner),
            people: RwSignal::new(people),
            next: RwSignal::new(next),
            loaded: RwSignal::new(loaded),
            failed: RwSignal::new(None),
            listed: StoredValue::new(listed),
        }
    }

    /// Add a page: its account, if it names it; its people not listed yet;
    /// and where the list goes on. The list is kept for this visit.
    pub fn add(self, page: &FollowsPayload) {
        if let Some(owner) = &page.owner {
            self.owner.set(Some(owner.clone()));
        }
        let fresh: Vec<Arc<Person>> = self
            .listed
            .try_update_value(|listed| {
                page.people.iter().filter(|p| listed.insert(p.fid)).map(|p| Arc::new(p.clone())).collect()
            })
            .unwrap_or_default();
        if !fresh.is_empty() {
            self.people.update(|people| people.extend(fresh));
        }
        self.next.set(page.next.clone());
        self.loaded.set(true);
        self.keep();
    }

    /// How many people are in.
    pub fn count(self) -> usize {
        self.people.with(Vec::len)
    }

    /// Whether every person of the list is in.
    pub fn complete(self) -> bool {
        self.loaded.get() && self.next.with(Option::is_none)
    }

    fn keep(self) {
        let Some(owner) = self.owner.get_untracked() else { return };
        let key = self.key.get_value();
        let kept = Kept {
            key: key.clone(),
            at: js_sys::Date::now(),
            owner,
            people: self.people.get_untracked(),
            next: self.next.get_untracked(),
        };
        KEPT.with(|lists| {
            let mut lists = lists.borrow_mut();
            lists.retain(|k| k.key != key);
            if lists.len() >= KEPT_LISTS {
                lists.remove(0);
            }
            lists.push(kept);
        });
    }
}

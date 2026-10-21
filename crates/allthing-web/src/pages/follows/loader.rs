// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reading a followers or following list a page at a time, one request
//! after another and never two at once, as long as the page wants more
//! people than are in ([`Loader::want`]): the list as it is scrolled, and
//! all of it, when a reader searches it. A later page that fails is tried
//! again after 2, 4 and 8 seconds, then waits for the reader's Try again
//! ([`Loader::retry`]); the people shown stay. While the tab is hidden,
//! nothing is read.

use allthing_core::api::{FollowsDir, FollowsPayload, follows_path};
use leptos::prelude::*;

use super::store::People;
use crate::api::{self, Failure};
use crate::util::document;

/// Tries of a later page before the reader is asked to try again.
const RETRIES: u32 = 3;

#[derive(Clone, Copy)]
pub struct Loader {
    people: People,
    name: StoredValue<String>,
    dir: FollowsDir,
    /// A page is on its way.
    pub loading: RwSignal<bool>,
    /// A later page failed, every try: the reader can try again.
    pub stuck: RwSignal<bool>,
    /// Read on until at least this many people are in, or the list ends.
    wanted: RwSignal<usize>,
    /// Bumped as the tab comes back into view.
    shown: RwSignal<u32>,
    failures: StoredValue<u32>,
    /// The page that failed every try, until it's tried again.
    failed: StoredValue<Option<String>>,
    /// Bumped as the page goes: an answer for an earlier one is dropped.
    revision: StoredValue<u64>,
    timer: StoredValue<Option<TimeoutHandle>>,
}

impl Loader {
    /// Read `name`'s `dir` list into `people`: its first page at once,
    /// unless the visit has it already, and later pages as they're wanted.
    pub fn new(people: People, name: String, dir: FollowsDir) -> Loader {
        let loader = Loader {
            people,
            name: StoredValue::new(name),
            dir,
            loading: RwSignal::new(false),
            stuck: RwSignal::new(false),
            wanted: RwSignal::new(1),
            shown: RwSignal::new(0),
            failures: StoredValue::new(0),
            failed: StoredValue::new(None),
            revision: StoredValue::new(0),
            timer: StoredValue::new(None),
        };
        let shown = window_event_listener_untyped("visibilitychange", move |_| {
            if !document().hidden() {
                let _ = loader.shown.try_update(|n| *n = n.wrapping_add(1));
            }
        });
        on_cleanup(move || {
            shown.remove();
            loader.revision.update_value(|r| *r = r.wrapping_add(1));
            if let Some(Some(handle)) = loader.timer.try_get_value() {
                handle.clear();
            }
        });
        // Read whenever more people are wanted than are in, and nothing
        // stands in the way.
        Effect::new(move |_| {
            loader.shown.track();
            let wanted = loader.wanted.get();
            let more = !people.loaded.get() || (people.next.with(Option::is_some) && people.count() < wanted);
            if more && !loader.loading.get() && !loader.stuck.get() && people.failed.with(Option::is_none) {
                loader.read();
            }
        });
        loader
    }

    /// Read on until at least `n` people are in (the list's end permitting).
    pub fn want(self, n: usize) {
        if n > self.wanted.get_untracked() {
            self.wanted.set(n);
        }
    }

    /// Try the page that failed again, past any answer kept for it.
    pub fn retry(self) {
        if let Some(path) = self.failed.try_update_value(Option::take).flatten() {
            api::forget_one(&path);
        }
        self.failures.set_value(0);
        self.stuck.set(false);
    }

    fn read(self) {
        if document().hidden() {
            return;
        }
        let people = self.people;
        let path = match (people.loaded.get_untracked(), people.next.get_untracked(), people.owner.get_untracked()) {
            (false, _, _) => follows_path(&self.name.get_value(), self.dir, None),
            (true, Some(cursor), Some(owner)) => follows_path(&format!("fid:{}", owner.fid), self.dir, Some(&cursor)),
            _ => return,
        };
        self.loading.set(true);
        let revision = self.revision.get_value();
        wasm_bindgen_futures::spawn_local(async move {
            let result = api::get::<FollowsPayload>(&path).await;
            // The page went meanwhile.
            if self.revision.try_get_value() != Some(revision) {
                return;
            }
            match result {
                Ok(page) => {
                    self.failures.set_value(0);
                    people.add(&page.data);
                }
                // The first page failing is the page's to say.
                Err(failure) if !people.loaded.get_untracked() => people.failed.set(Some(failure)),
                Err(failure) => {
                    let tries = self.failures.get_value() + 1;
                    self.failures.set_value(tries);
                    if tries > RETRIES || matches!(failure, Failure::NotFound(_)) {
                        self.failed.set_value(Some(path));
                        self.stuck.set(true);
                    } else {
                        // Again in 2, 4, then 8 seconds: `loading` stays on
                        // until then, so nothing else asks meanwhile.
                        let wait = std::time::Duration::from_secs(1 << tries);
                        let handle = set_timeout_with_handle(
                            move || {
                                if self.revision.try_get_value() == Some(revision) {
                                    self.timer.set_value(None);
                                    self.loading.set(false);
                                }
                            },
                            wait,
                        );
                        self.timer.set_value(handle.ok());
                        return;
                    }
                }
            }
            self.loading.set(false);
        });
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! Likes, recasts and replies kept current while they are in view: as
//! current as the reader's notifications, which come from the same node.
//!
//! - **What is asked about**: the run of [`ENGAGEMENT_CHUNK`] feed cards each
//!   card in view belongs to (the same runs `preload` asks engagement
//!   batches for, so readers of a feed share each run's edge cache entry),
//!   and the post a post page shows with its first replies ([`watch`]).
//! - **How often**: every [`EVERY_MS`] while the page is in view and the
//!   reader has scrolled, pointed or typed in the last [`IDLE_MS`], and when
//!   the page comes back into view, a run not read for [`AGAIN_MS`] is read
//!   again through `/api/v1/counts` (one bulk read of at most
//!   [`allthing_core::api::COUNTS_MAX`] casts at the Worker, fresh there for
//!   15 s), at most [`PER_TICK`] at a time. A page just read is as fresh, so
//!   the first read is a tick after it shows. A tab left alone stops asking.
//!   Nothing on Save-Data or 2g.
//! - **How they show**: [`likes`], [`recasts`] and [`replies`] give a count as
//!   last read here, or as the page read it. The writer's own merge
//!   (`Writer::shown_count`, `replies_shown`) still applies on top, so the
//!   reader's pending likes and replies are never undone.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;

use allthing_core::api::{COUNTS_MAX, Counts, CountsPayload, ENGAGEMENT_CHUNK, counts_path};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::util::document;

/// How often what is in view is looked at again.
const EVERY_MS: u64 = 30_000;
/// A run read this recently isn't read again.
const AGAIN_MS: f64 = 25_000.0;
/// Runs read at most at a time.
const PER_TICK: usize = 2;
/// After the page comes back into view, wait this long.
const SETTLE_MS: u64 = 600;
/// With nothing done for this long, the page is left alone: no more reads.
const IDLE_MS: f64 = 120_000.0;

thread_local! {
    static FRESH: RefCell<Option<RwSignal<HashMap<String, Counts>>>> = const { RefCell::new(None) };
    /// When each `/counts` path was last asked for, Unix milliseconds.
    static ASKED: RefCell<HashMap<String, f64>> = RefCell::new(HashMap::new());
    /// What post pages watch, by watcher.
    static WATCHED: RefCell<Vec<(u64, Vec<String>)>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
    static PENDING: Cell<bool> = const { Cell::new(false) };
    /// When the reader last scrolled, pointed or typed, Unix milliseconds.
    static ACTIVE: Cell<f64> = const { Cell::new(0.0) };
}

/// Start keeping counts current: once, as the app mounts.
pub fn start() {
    FRESH.with(|f| *f.borrow_mut() = Some(RwSignal::new(HashMap::new())));
    ACTIVE.with(|a| a.set(js_sys::Date::now()));
    set_interval(tick, Duration::from_millis(EVERY_MS));
    let _ = window_event_listener_untyped("visibilitychange", |_| {
        ACTIVE.with(|a| a.set(js_sys::Date::now()));
        soon();
    });
    for name in ["scroll", "pointerdown", "pointermove", "keydown"] {
        let _ = window_event_listener_untyped(name, |_| ACTIVE.with(|a| a.set(js_sys::Date::now())));
    }
}

fn fresh() -> Option<RwSignal<HashMap<String, Counts>>> {
    FRESH.with(|f| *f.borrow())
}

fn current(hash: &str) -> Option<Counts> {
    let hash = hash.to_ascii_lowercase();
    fresh().and_then(|signal| signal.with(|map| map.get(&hash).copied()))
}

/// `hash`'s likes: as last read here, or `read`, as the page has it
/// (tracked).
pub fn likes(hash: &str, read: u64) -> u64 {
    current(hash).map_or(read, |c| c.likes)
}

pub fn recasts(hash: &str, read: u64) -> u64 {
    current(hash).map_or(read, |c| c.recasts)
}

pub fn replies(hash: &str, read: u64) -> u64 {
    current(hash).map_or(read, |c| c.replies)
}

/// Keep `hashes` (a post and its first replies) current while the calling
/// component lives.
pub fn watch(hashes: Vec<String>) {
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    WATCHED.with(|w| w.borrow_mut().push((id, hashes)));
    on_cleanup(move || WATCHED.with(|w| w.borrow_mut().retain(|(watcher, _)| *watcher != id)));
}

/// Look at what is in view shortly: the page came back into view.
fn soon() {
    if PENDING.with(|p| p.replace(true)) {
        return;
    }
    set_timeout(
        || {
            PENDING.with(|p| p.set(false));
            tick();
        },
        Duration::from_millis(SETTLE_MS),
    );
}

/// The `/counts` paths for what is in view: the watched posts' first, then
/// the runs of the feed cards in view.
fn in_view() -> Vec<String> {
    let mut paths: Vec<String> = WATCHED.with(|w| {
        w.borrow()
            .iter()
            .flat_map(|(_, hashes)| hashes.chunks(COUNTS_MAX).filter_map(|c| counts_path(c.iter().map(String::as_str))))
            .collect()
    });
    for run in crate::preload::runs_in_view(ENGAGEMENT_CHUNK) {
        if let Some(path) = counts_path(run.iter().map(String::as_str))
            && !paths.contains(&path)
        {
            paths.push(path);
        }
    }
    paths
}

fn tick() {
    if document().hidden() || api::constrained_network() || fresh().is_none() {
        return;
    }
    let now = js_sys::Date::now();
    if now - ACTIVE.with(Cell::get) > IDLE_MS {
        return;
    }
    let due: Vec<String> = ASKED.with(|asked| {
        let mut asked = asked.borrow_mut();
        asked.retain(|_, at| now - *at < AGAIN_MS);
        let due: Vec<String> = in_view().into_iter().filter(|p| !asked.contains_key(p)).take(PER_TICK).collect();
        for path in &due {
            asked.insert(path.clone(), now);
        }
        due
    });
    for path in due {
        spawn_local(async move {
            if let Ok(fetched) = api::get::<CountsPayload>(&path).await {
                merge(&fetched.data.counts);
            }
        });
    }
}

/// File what a read brought, redrawing only when a count changed.
fn merge(counts: &std::collections::BTreeMap<String, Counts>) {
    let Some(signal) = fresh() else { return };
    let changed = signal.with_untracked(|map| counts.iter().any(|(hash, c)| map.get(hash) != Some(c)));
    if changed {
        signal.update(|map| map.extend(counts.iter().map(|(hash, c)| (hash.clone(), *c))));
    }
}

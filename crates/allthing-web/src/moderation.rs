// SPDX-License-Identifier: AGPL-3.0-or-later
//! Private account filters on the verified notification connection. A fresh
//! signed-in session waits for its authoritative snapshot before drawing content.
//! Per-field revisions and a committed IndexedDB outbox preserve independent edits.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use allthing_core::model::{Cast, UserSummary};
use allthing_core::moderation::FilterSet;
use allthing_core::notifications::{Group, GroupTime};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncStatus {
    SignedOut,
    Connecting,
    Synced,
    Saving,
    Pending,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preference {
    pub target_fid: u64,
    pub username: String,
    pub muted: bool,
    pub blocked: bool,
    pub pending: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Row {
    pub fid: u64,
    pub target_fid: u64,
    #[serde(default)]
    pub target_username: String,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub mute_revision: u64,
    #[serde(default)]
    pub block_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Intent {
    pub sequence: u64,
    pub desired: bool,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stored {
    pub server: Row,
    #[serde(default)]
    pub known: bool,
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub settled_mute: u64,
    #[serde(default)]
    pub settled_block: u64,
    #[serde(default)]
    pub pending_mute: Option<Intent>,
    #[serde(default)]
    pub pending_block: Option<Intent>,
    #[serde(default)]
    pub aliases: BTreeSet<String>,
}

impl Stored {
    pub(crate) fn merge_disk(&mut self, incoming: &Self) {
        if self.server.fid != incoming.server.fid || self.server.target_fid != incoming.server.target_fid {
            return;
        }
        if incoming.known {
            if !self.known || incoming.server.mute_revision > self.server.mute_revision {
                self.server.muted = incoming.server.muted;
                self.server.mute_revision = incoming.server.mute_revision;
            }
            if !self.known || incoming.server.block_revision > self.server.block_revision {
                self.server.blocked = incoming.server.blocked;
                self.server.block_revision = incoming.server.block_revision;
            }
            if !incoming.server.target_username.is_empty()
                && incoming.server.mute_revision >= self.server.mute_revision
                && incoming.server.block_revision >= self.server.block_revision
            {
                self.server.target_username.clone_from(&incoming.server.target_username);
            }
            self.known = true;
        }
        self.sequence = self.sequence.max(incoming.sequence);
        self.aliases.extend(incoming.aliases.iter().cloned());
        if !self.server.target_username.is_empty() {
            self.aliases.insert(self.server.target_username.clone());
        }
        self.settled_mute = self.settled_mute.max(incoming.settled_mute);
        self.settled_block = self.settled_block.max(incoming.settled_block);
        self.pending_mute = merge_intent(&self.pending_mute, &incoming.pending_mute, self.settled_mute);
        self.pending_block = merge_intent(&self.pending_block, &incoming.pending_block, self.settled_block);
        // Any newer authoritative revision supersedes an old intent, including
        // a lost acknowledgement followed by another device's undo. Never
        // rebase and replay that stale intent over the newer choice.
        self.settle_superseded();
    }
    fn settle_superseded(&mut self) {
        if self.known {
            for kind in [0, 1] {
                if let Some(p) = self.intent(kind).clone().filter(|p| self.revision(kind) > p.expected_revision) {
                    self.acknowledge(kind, &p);
                }
            }
        }
    }
    pub(crate) fn intent(&self, kind: u8) -> &Option<Intent> {
        if kind == 0 { &self.pending_mute } else { &self.pending_block }
    }
    fn intent_mut(&mut self, kind: u8) -> &mut Option<Intent> {
        if kind == 0 { &mut self.pending_mute } else { &mut self.pending_block }
    }
    fn revision(&self, kind: u8) -> u64 {
        if kind == 0 { self.server.mute_revision } else { self.server.block_revision }
    }
    fn value(&self, kind: u8) -> bool {
        if kind == 0 { self.server.muted } else { self.server.blocked }
    }
    #[cfg(test)]
    fn set_intent(&mut self, kind: u8, desired: bool) -> Result<(), String> {
        self.set_intent_at(kind, desired, self.revision(kind))
    }
    pub(crate) fn set_intent_at(&mut self, kind: u8, desired: bool, expected_revision: u64) -> Result<(), String> {
        if kind > 1 {
            return Err("Invalid account filter kind".into());
        }
        self.sequence = self.sequence.checked_add(1).ok_or("Account preference sequence exhausted")?;
        *self.intent_mut(kind) = Some(Intent { sequence: self.sequence, desired, expected_revision });
        // IDB may already contain a newer authoritative row than the browser
        // had when the reader clicked. Allocate under its transaction lock,
        // but never silently rebase that click over the unseen newer choice.
        self.settle_superseded();
        Ok(())
    }
    fn acknowledge(&mut self, kind: u8, intent: &Intent) {
        let settled = if kind == 0 { &mut self.settled_mute } else { &mut self.settled_block };
        *settled = (*settled).max(intent.sequence);
        if self.intent(kind).as_ref().is_some_and(|p| p.sequence <= intent.sequence) {
            *self.intent_mut(kind) = None;
        }
    }
    fn pending(&self) -> bool {
        self.pending_mute.is_some() || self.pending_block.is_some()
    }
    // Removing a filter reveals content only after acknowledgement. Adding one
    // hides at once, including while offline or waiting for its disk commit.
    fn hides(&self) -> bool {
        self.server.muted
            || self.server.blocked
            || self.pending_mute.as_ref().is_some_and(|p| p.desired)
            || self.pending_block.as_ref().is_some_and(|p| p.desired)
    }
}

fn merge_intent(a: &Option<Intent>, b: &Option<Intent>, settled: u64) -> Option<Intent> {
    a.iter().chain(b.iter()).filter(|p| p.sequence > settled).max_by_key(|p| (p.sequence, p.expected_revision)).cloned()
}

#[derive(Clone)]
struct State {
    fid: Option<u64>,
    epoch: u64,
    capability: Option<bool>,
    hydrated: bool,
    disk_loaded: bool,
    paused: bool,
    status: SyncStatus,
    rows: BTreeMap<u64, Stored>,
    staged: BTreeMap<(u64, u8), (String, bool, u64, u64)>,
    stage_sequence: u64,
    saving: BTreeSet<(u64, u8)>,
    filters: FilterSet,
}
impl Default for State {
    fn default() -> Self {
        Self {
            fid: None,
            epoch: 0,
            capability: None,
            hydrated: false,
            disk_loaded: false,
            paused: false,
            status: SyncStatus::SignedOut,
            rows: BTreeMap::new(),
            staged: BTreeMap::new(),
            stage_sequence: 0,
            saving: BTreeSet::new(),
            filters: FilterSet::default(),
        }
    }
}
impl State {
    fn rebuild(&mut self) {
        let mut filters = FilterSet::default();
        for item in self.rows.values().filter(|item| item.hides()) {
            filters.insert(item.server.target_fid, &item.server.target_username);
            for alias in &item.aliases {
                filters.insert(item.server.target_fid, alias);
            }
        }
        for ((fid, _), (name, desired, _, _)) in &self.staged {
            if *desired {
                filters.insert(*fid, name);
            }
        }
        self.filters = filters;
    }
    fn waiting(&self) -> bool {
        self.fid.is_some()
            && (!self.disk_loaded
                || match self.capability {
                    Some(false) => self.rows.values().any(Stored::hides),
                    _ => !self.hydrated,
                })
    }
    fn pending(&self) -> bool {
        !self.staged.is_empty() || self.rows.values().any(Stored::pending)
    }
    fn waiting_for(&self, current_fid: Option<u64>) -> bool {
        self.fid != current_fid || self.waiting()
    }
    #[cfg(any(test, not(feature = "static-host")))]
    fn stage(&mut self, target: u64, name: &str, kind: u8, desired: bool) -> Result<(), String> {
        if kind > 1 {
            return Err("Invalid account filter kind".into());
        }
        self.stage_sequence = self.stage_sequence.checked_add(1).ok_or("Account preference sequence exhausted")?;
        let expected_revision = self.rows.get(&target).map_or(0, |item| item.revision(kind));
        self.staged.insert((target, kind), (name.to_owned(), desired, self.stage_sequence, expected_revision));
        self.paused = false;
        self.status = SyncStatus::Saving;
        self.rebuild();
        Ok(())
    }
}

/// Only privacy changes invalidate rendered content. Sync status and outbox
/// bookkeeping must not replace a focused editor or a visible thread.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Visibility {
    waiting: bool,
    filters: FilterSet,
    blocked: BTreeSet<u64>,
}
impl Visibility {
    fn of(state: &State, current: Option<u64>) -> Self {
        let blocked = state
            .rows
            .iter()
            .filter(|(_, row)| row.server.blocked || row.pending_block.as_ref().is_some_and(|p| p.desired))
            .map(|(fid, _)| *fid)
            .chain(
                state
                    .staged
                    .iter()
                    .filter(|((_, kind), (_, desired, _, _))| *kind == 1 && *desired)
                    .map(|((fid, _), _)| *fid),
            )
            .collect();
        Self { waiting: state.waiting_for(current), filters: state.filters.clone(), blocked }
    }
}

#[derive(Clone, Copy)]
pub struct Moderation(RwSignal<State>, Signal<Option<u64>>, Memo<Visibility>);
thread_local! { static HANDLE: Cell<Option<Moderation>> = const { Cell::new(None) }; }
pub fn use_moderation() -> Moderation {
    use_context::<Moderation>().expect("Account filters context")
}
fn handle() -> Option<Moderation> {
    HANDLE.with(Cell::get)
}

impl Moderation {
    pub fn new() -> Self {
        #[cfg(not(feature = "static-host"))]
        let current = {
            let session = crate::components::signin::use_session();
            Signal::derive(move || session.user.with(|user| user.as_ref().map(|user| user.fid)))
        };
        #[cfg(feature = "static-host")]
        let current = Signal::stored(None);
        let state = RwSignal::new(State::default());
        let visibility = Memo::new(move |_| state.with(|state| Visibility::of(state, current.get())));
        let m = Self(state, current, visibility);
        HANDLE.with(|h| h.set(Some(m)));
        #[cfg(not(feature = "static-host"))]
        {
            // Establish the privacy barrier before any page can render a
            // cached card, rather than waiting for Notifications' effect.
            account(current.get_untracked());
            let _ = window_event_listener(leptos::ev::storage, move |ev| {
                if ev.key().as_deref() == Some("allthing.account-filters.changed") {
                    reload_disk(m);
                }
            });
        }
        m
    }
    pub fn signed_in(self) -> bool {
        self.1.get().is_some()
    }
    fn waiting(self) -> bool {
        self.2.with(|visibility| visibility.waiting)
    }
    pub fn status(self) -> SyncStatus {
        if self.1.get() != self.0.with(|s| s.fid) {
            return SyncStatus::Connecting;
        }
        self.0.with(|s| s.status.clone())
    }
    pub fn preference(self, fid: u64) -> Preference {
        if self.1.get() != self.0.with(|s| s.fid) {
            return Preference { target_fid: fid, pending: true, ..Preference::default() };
        }
        self.0.with(|s| {
            let row = s.rows.get(&fid);
            let pending = row.is_some_and(Stored::pending) || s.staged.keys().any(|(target, _)| *target == fid);
            Preference {
                target_fid: fid,
                username: row
                    .map(|r| r.server.target_username.clone())
                    .or_else(|| {
                        s.staged.iter().find(|((target, _), _)| *target == fid).map(|(_, value)| value.0.clone())
                    })
                    .unwrap_or_default(),
                // Labels describe a durable committed outbox/server record.
                // Staging still hides immediately, but Saving does not claim
                // that a reload can recover a transaction not yet committed.
                muted: row.is_some_and(|r| r.pending_mute.as_ref().map_or(r.server.muted, |p| p.desired)),
                blocked: row.is_some_and(|r| r.pending_block.as_ref().map_or(r.server.blocked, |p| p.desired)),
                pending,
            }
        })
    }
    pub fn preferences(self) -> Vec<Preference> {
        if self.1.get() != self.0.with(|s| s.fid) {
            return Vec::new();
        }
        let fids = self
            .0
            .with(|s| s.rows.keys().copied().chain(s.staged.keys().map(|(fid, _)| *fid)).collect::<BTreeSet<_>>());
        fids.into_iter().map(|fid| self.preference(fid)).filter(|p| p.muted || p.blocked || p.pending).collect()
    }
    pub fn set_muted(self, fid: u64, username: &str, desired: bool) {
        self.change(fid, username, 0, desired);
    }
    pub fn set_blocked(self, fid: u64, username: &str, desired: bool) {
        self.change(fid, username, 1, desired);
    }
    fn change(self, target: u64, name: &str, kind: u8, desired: bool) {
        #[cfg(not(feature = "static-host"))]
        {
            let valid = self.0.with_untracked(|s| {
                s.fid.is_some_and(|fid| target > 0 && target != fid)
                    && self.1.get_untracked() == s.fid
                    && s.hydrated
                    && s.disk_loaded
                    && s.capability == Some(true)
            });
            if !valid {
                self.0.update(|s| {
                    s.status =
                        SyncStatus::Failed("Connect your private account preferences before changing a filter.".into())
                });
                return;
            }
            self.0.update(|s| {
                if let Err(error) = s.stage(target, name, kind, desired) {
                    s.status = SyncStatus::Failed(error);
                }
            });
            persist_staged(self);
        }
        #[cfg(feature = "static-host")]
        let _ = (target, name, kind, desired);
    }
    pub fn retry(self) {
        #[cfg(not(feature = "static-host"))]
        {
            self.0.update(|s| s.paused = false);
            retry_storage();
            persist_staged(self);
            crate::notification_sync::retry_preferences();
        }
    }
}

pub fn waiting() -> bool {
    handle().is_some_and(Moderation::waiting)
}
pub fn hides_cast(cast: &Cast, quotes: &BTreeMap<String, Cast>) -> bool {
    handle().is_some_and(|m| m.2.with(|v| v.waiting || v.filters.hides_cast(cast, quotes)))
}
pub fn hides_user(user: &UserSummary) -> bool {
    handle().is_some_and(|m| m.2.with(|v| v.waiting || v.filters.hides_user(user)))
}
pub fn hides_text(text: &str) -> bool {
    handle().is_some_and(|m| m.2.with(|v| v.waiting || v.filters.hides_text(text)))
}
pub fn hides_references(references: &allthing_core::moderation::References) -> bool {
    handle().is_some_and(|m| m.2.with(|v| v.waiting || v.filters.hides_references(references)))
}
pub fn hides_group(group: &Group) -> bool {
    handle().is_some_and(|m| m.2.with(|v| v.waiting || v.filters.hides_group(group)))
}
pub fn hides_group_time(group: &GroupTime) -> bool {
    handle().is_some_and(|m| m.2.with(|v| v.waiting || v.filters.hides_group_time(group)))
}
pub fn blocks_fid(fid: u64) -> bool {
    handle().is_some_and(|m| m.2.with(|v| v.waiting || v.blocked.contains(&fid)))
}

#[component]
pub fn FilterNotice() -> impl IntoView {
    let m = use_moderation();
    view! { <p class="account-filters-waiting" role="status">
        {move || match m.status() { SyncStatus::Failed(why) => format!("Posts stay hidden until your private account filters reconnect. {why}"), _ => "Syncing your private account filters before showing posts…".to_owned() }}
        <button type="button" class="text-button" on:click=move |_| m.retry()>"Retry sync"</button>
    </p> }
}

#[cfg(not(feature = "static-host"))]
mod wire {
    use super::*;
    use std::cell::RefCell;
    use wasm_bindgen::JsValue;
    #[derive(Clone)]
    pub(super) struct Binding {
        pub fid: u64,
        pub epoch: u64,
        pub attempt: u64,
        pub bridge: JsValue,
    }
    thread_local! { pub(super) static BINDING: RefCell<Option<Binding>> = const { RefCell::new(None) }; pub(super) static WRITING: Cell<bool> = const { Cell::new(false) }; }
}

#[cfg(not(feature = "static-host"))]
fn same(m: Moderation, fid: u64, epoch: u64) -> bool {
    m.1.get_untracked() == Some(fid) && m.0.with_untracked(|s| s.fid == Some(fid) && s.epoch == epoch)
}

#[cfg(not(feature = "static-host"))]
pub fn account(fid: Option<u64>) {
    let Some(m) = handle() else { return };
    if m.0.with_untracked(|s| s.fid) == fid {
        return;
    }
    wire::BINDING.with(|b| b.borrow_mut().take());
    let epoch = m.0.with_untracked(|s| s.epoch.wrapping_add(1));
    m.0.set(State {
        fid,
        epoch,
        status: if fid.is_some() { SyncStatus::Connecting } else { SyncStatus::SignedOut },
        ..State::default()
    });
    reload_disk(m);
}

#[cfg(not(feature = "static-host"))]
pub fn capability(fid: u64, enabled: bool) {
    if let Some(m) = handle() {
        m.0.update(|s| {
            if s.fid == Some(fid) {
                s.capability = Some(enabled);
                if !enabled {
                    s.hydrated = true;
                    s.status = SyncStatus::Failed("Private account filters are not configured on this server.".into());
                }
            }
        });
    }
}

#[cfg(not(feature = "static-host"))]
pub fn bind(fid: u64, attempt: u64, bridge: wasm_bindgen::JsValue) {
    let Some(m) = handle() else { return };
    let epoch = m.0.with_untracked(|s| s.epoch);
    if !same(m, fid, epoch) {
        return;
    }
    wire::BINDING.with(|b| *b.borrow_mut() = Some(wire::Binding { fid, epoch, attempt, bridge }));
}

#[cfg(not(feature = "static-host"))]
fn snapshot(binding: &wire::Binding) -> Result<Vec<Row>, String> {
    use crate::stdb_bridge::{js_error, method, parse};
    parse(method(&binding.bridge, "moderationState")?.call0(&binding.bridge).map_err(js_error)?)
}

#[cfg(not(feature = "static-host"))]
pub fn base_applied(fid: u64) -> Result<(), String> {
    let m = handle().ok_or("Account filters are not mounted")?;
    let b = wire::BINDING
        .with(|b| b.borrow().clone())
        .filter(|b| b.fid == fid)
        .ok_or("Account filters are not connected")?;
    let rows = snapshot(&b)?;
    if rows.iter().any(|r| r.fid != fid || r.target_fid == 0 || r.target_fid == fid) {
        return Err("Account filter snapshot belongs to another account".into());
    }
    for row in rows {
        observe(m, row);
    }
    m.0.update(|s| {
        if s.fid == Some(fid) {
            s.hydrated = true;
            if !s.pending() {
                s.paused = false;
            }
            s.status = if !s.disk_loaded {
                SyncStatus::Connecting
            } else if !s.staged.is_empty() {
                SyncStatus::Saving
            } else if s.pending() {
                SyncStatus::Pending
            } else {
                SyncStatus::Synced
            };
        }
    });
    reload_disk(m);
    flush();
    Ok(())
}

#[cfg(not(feature = "static-host"))]
pub fn connection_failed(fid: u64, why: String) {
    wire::BINDING.with(|b| {
        if b.borrow().as_ref().is_some_and(|b| b.fid == fid) {
            b.borrow_mut().take();
        }
    });
    if let Some(m) = handle() {
        m.0.update(|s| {
            if s.fid == Some(fid) {
                s.status = SyncStatus::Failed(why);
            }
        });
    }
}

#[cfg(not(feature = "static-host"))]
pub fn event(fid: u64, json: &str) {
    #[derive(Deserialize)]
    struct Event {
        t: String,
        #[serde(default)]
        table: String,
        row: Option<Row>,
    }
    let Ok(e) = serde_json::from_str::<Event>(json) else { return };
    if e.table != "my_account_filters" || !matches!(e.t.as_str(), "insert" | "update") {
        return;
    }
    if let (Some(m), Some(row)) = (handle(), e.row.filter(|r| r.fid == fid && r.target_fid != fid && r.target_fid > 0))
    {
        observe(m, row);
    }
}

#[cfg(not(feature = "static-host"))]
fn observe(m: Moderation, row: Row) {
    if m.1.get_untracked() != Some(row.fid) || m.0.with_untracked(|s| s.fid) != Some(row.fid) {
        return;
    }
    let item = Stored { server: row.clone(), known: true, ..Stored::default() };
    m.0.update(|s| {
        s.rows.entry(row.target_fid).or_insert_with(|| item.clone()).merge_disk(&item);
        s.rebuild();
        if s.hydrated && s.disk_loaded && !s.pending() {
            s.status = SyncStatus::Synced;
            s.paused = false;
        }
    });
    persist_item(m, row.target_fid);
}

#[cfg(not(feature = "static-host"))]
fn signal_disk() {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item("allthing.account-filters.changed", &js_sys::Date::now().to_string());
    }
}

#[cfg(not(feature = "static-host"))]
pub(crate) fn retry_storage() {
    if let Some(m) = handle() {
        // Retry the same account/epoch-fenced merge without discarding cached
        // filters or assuming a failed storage read returned an empty list.
        reload_disk(m);
    }
}

#[cfg(not(feature = "static-host"))]
fn reload_disk(m: Moderation) {
    let (fid, epoch) = m.0.with_untracked(|s| (s.fid, s.epoch));
    let Some(fid) = fid else { return };
    leptos::task::spawn_local(async move {
        match crate::library_disk::moderation_items(fid).await {
            Ok(items) if same(m, fid, epoch) => {
                m.0.update(|s| {
                    for item in items {
                        s.rows.entry(item.server.target_fid).or_insert_with(|| item.clone()).merge_disk(&item);
                    }
                    s.disk_loaded = true;
                    s.rebuild();
                    if s.hydrated && !s.paused && s.capability == Some(true) {
                        s.status = if !s.staged.is_empty() {
                            SyncStatus::Saving
                        } else if s.pending() {
                            SyncStatus::Pending
                        } else {
                            SyncStatus::Synced
                        };
                    }
                });
                flush();
            }
            Err(e) if same(m, fid, epoch) => m.0.update(|s| {
                s.status = SyncStatus::Failed(format!("Account preferences could not be read on this device: {e}"))
            }),
            _ => {}
        }
    });
}

#[cfg(not(feature = "static-host"))]
fn persist_item(m: Moderation, target: u64) {
    let (fid, epoch, item) = m.0.with_untracked(|s| (s.fid, s.epoch, s.rows.get(&target).cloned()));
    let (Some(fid), Some(item)) = (fid, item) else { return };
    leptos::task::spawn_local(async move {
        match crate::library_disk::keep_moderation(fid, &item, None).await {
            Ok(saved) => {
                signal_disk();
                if same(m, fid, epoch) {
                    m.0.update(|s| {
                        s.rows.entry(target).or_insert_with(|| saved.clone()).merge_disk(&saved);
                        s.rebuild();
                    });
                }
            }
            Err(e) if same(m, fid, epoch) => m.0.update(|s| {
                s.status = SyncStatus::Failed(format!("Account preferences could not be saved on this device: {e}"))
            }),
            _ => {}
        }
    });
}

#[cfg(not(feature = "static-host"))]
fn persist_staged(m: Moderation) {
    let (fid, epoch, staged) = m.0.with_untracked(|s| (s.fid, s.epoch, s.staged.clone()));
    let Some(fid) = fid else { return };
    for ((target, kind), (name, desired, token, expected_revision)) in staged {
        let already = m.0.with_untracked(|s| s.saving.contains(&(target, kind)));
        if already {
            continue;
        }
        m.0.update(|s| {
            s.saving.insert((target, kind));
        });
        let mut item = m.0.with_untracked(|s| s.rows.get(&target).cloned()).unwrap_or_else(|| Stored {
            server: Row { fid, target_fid: target, ..Row::default() },
            known: true,
            ..Stored::default()
        });
        item.server.target_username = name.clone();
        item.aliases.insert(name);
        leptos::task::spawn_local(async move {
            match crate::library_disk::keep_moderation(fid, &item, Some((kind, desired, expected_revision))).await {
                Ok(saved) => {
                    signal_disk();
                    if same(m, fid, epoch) {
                        m.0.update(|s| {
                            s.rows.entry(target).or_insert_with(|| saved.clone()).merge_disk(&saved);
                            if s.staged.get(&(target, kind)).is_some_and(|p| p.2 == token) {
                                s.staged.remove(&(target, kind));
                            }
                            s.saving.remove(&(target, kind));
                            s.rebuild();
                            if !s.paused {
                                s.status = if !s.staged.is_empty() {
                                    SyncStatus::Saving
                                } else if s.pending() {
                                    SyncStatus::Pending
                                } else {
                                    SyncStatus::Synced
                                };
                            }
                        });
                        persist_staged(m);
                        flush();
                    }
                }
                Err(e) if same(m, fid, epoch) => m.0.update(|s| {
                    s.saving.remove(&(target, kind));
                    s.status = SyncStatus::Failed(format!("Not saved on this device; retry to sync your filter: {e}"));
                }),
                _ => {}
            }
        });
    }
}

#[cfg(not(feature = "static-host"))]
fn flush() {
    use crate::stdb_bridge::{js_error, method, parse};
    use allthing_core::stdb::Called;
    let Some(m) = handle() else { return };
    let Some(binding) = wire::BINDING.with(|b| b.borrow().clone()) else { return };
    if !same(m, binding.fid, binding.epoch)
        || !m.0.with_untracked(|s| s.hydrated && s.disk_loaded && !s.paused)
        || wire::WRITING.with(|w| w.replace(true))
    {
        return;
    }
    leptos::task::spawn_local(async move {
        loop {
            if !same(m, binding.fid, binding.epoch)
                || !wire::BINDING.with(|b| {
                    b.borrow().as_ref().is_some_and(|b| {
                        b.attempt == binding.attempt && b.fid == binding.fid && b.epoch == binding.epoch
                    })
                })
            {
                break;
            }
            let next = m.0.with_untracked(|s| {
                s.rows.values().find_map(|item| {
                    [0, 1].into_iter().find_map(|kind| {
                        item.intent(kind)
                            .clone()
                            .map(|intent| (item.server.target_fid, item.server.target_username.clone(), kind, intent))
                    })
                })
            });
            let Some((target, name, kind, intent)) = next else {
                m.0.update(|s| {
                    if s.staged.is_empty() {
                        s.status = SyncStatus::Synced;
                    }
                });
                break;
            };
            let args = js_sys::Array::new();
            for value in [
                wasm_bindgen::JsValue::from(target as f64),
                name.into(),
                (kind as f64).into(),
                intent.desired.into(),
                (intent.expected_revision as f64).into(),
            ] {
                args.push(&value);
            }
            let result = async {
                let pending =
                    method(&binding.bridge, "setAccountFilter")?.apply(&binding.bridge, &args).map_err(js_error)?;
                parse::<Called>(crate::notification_sync::bounded(pending).await?)?.result()?;
                let rows = snapshot(&binding)?;
                let row = rows
                    .into_iter()
                    .find(|r| r.fid == binding.fid && r.target_fid == target)
                    .ok_or("Account filter was not confirmed")?;
                let mut saved =
                    m.0.with_untracked(|s| s.rows.get(&target).cloned()).ok_or("Account filter was forgotten")?;
                saved.merge_disk(&Stored { server: row, known: true, ..Stored::default() });
                if saved.value(kind) != intent.desired {
                    return Err("Account preference changed in another session. Review it and retry.".into());
                }
                saved.acknowledge(kind, &intent);
                crate::library_disk::keep_moderation(binding.fid, &saved, None).await
            }
            .await;
            if !same(m, binding.fid, binding.epoch)
                || !wire::BINDING.with(|b| {
                    b.borrow().as_ref().is_some_and(|b| {
                        b.attempt == binding.attempt && b.epoch == binding.epoch && b.fid == binding.fid
                    })
                })
            {
                break;
            }
            match result {
                Ok(saved) => {
                    m.0.update(|s| {
                        s.rows.entry(target).or_insert_with(|| saved.clone()).merge_disk(&saved);
                        s.rebuild();
                        s.status = if !s.staged.is_empty() {
                            SyncStatus::Saving
                        } else if s.pending() {
                            SyncStatus::Pending
                        } else {
                            SyncStatus::Synced
                        };
                    });
                    signal_disk();
                }
                Err(e) => {
                    if m.0.with_untracked(|s| {
                        s.rows
                            .get(&target)
                            .and_then(|r| r.intent(kind).as_ref())
                            .is_none_or(|p| p.sequence != intent.sequence)
                    }) {
                        continue;
                    }
                    m.0.update(|s| {
                        s.paused = true;
                        s.status = SyncStatus::Failed(format!("Filter not synced; review and retry: {e}"));
                    });
                    break;
                }
            }
        }
        wire::WRITING.with(|w| w.set(false));
        if !same(m, binding.fid, binding.epoch)
            || wire::BINDING.with(|b| b.borrow().as_ref().is_some_and(|b| b.attempt != binding.attempt))
        {
            flush();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stored() -> Stored {
        Stored { server: Row { fid: 1, target_fid: 2, ..Row::default() }, known: true, ..Stored::default() }
    }
    #[test]
    fn independent_changes_and_tombstones_merge_without_reverting_newer_fields() {
        let mut a = stored();
        a.server.muted = true;
        a.server.mute_revision = 2;
        let mut b = stored();
        b.server.blocked = true;
        b.server.block_revision = 3;
        a.merge_disk(&b);
        assert!(a.server.muted && a.server.blocked);
        b.server.muted = false;
        b.server.mute_revision = 4;
        a.merge_disk(&b);
        a.merge_disk(&stored());
        assert!(!a.server.muted && a.server.blocked && a.server.mute_revision == 4);
    }
    #[test]
    fn late_ack_does_not_remove_later_intent_or_another_field() {
        let mut a = stored();
        a.set_intent(0, true).unwrap();
        let first = a.pending_mute.clone().unwrap();
        a.set_intent(0, false).unwrap();
        a.set_intent(1, true).unwrap();
        a.acknowledge(0, &first);
        assert_eq!(a.pending_mute.as_ref().map(|p| p.desired), Some(false));
        assert!(a.pending_block.is_some());
        let stale = Stored { pending_mute: Some(first.clone()), ..stored() };
        a.merge_disk(&stale);
        assert_eq!(a.pending_mute.as_ref().unwrap().sequence, 2);
        let last = a.pending_mute.clone().unwrap();
        a.acknowledge(0, &last);
        a.merge_disk(&stale);
        assert!(a.pending_mute.is_none());
    }
    #[test]
    fn unacknowledged_removal_cannot_flash_previously_hidden_content() {
        let mut a = stored();
        a.server.blocked = true;
        a.set_intent(1, false).unwrap();
        assert!(a.hides());
        a.server.blocked = false;
        let p = a.pending_block.clone().unwrap();
        a.acknowledge(1, &p);
        assert!(!a.hides());
        a.set_intent(0, true).unwrap();
        assert!(a.hides());
    }
    #[test]
    fn a_click_cannot_rebase_over_a_newer_authoritative_disk_record() {
        let mut incoming = stored();
        incoming.server.mute_revision = 1;
        incoming.server.muted = true;
        let clicked_revision = incoming.revision(0);
        let mut disk = incoming.clone();
        disk.sequence = 9;
        disk.server.mute_revision = 2;
        disk.server.muted = false;
        disk.merge_disk(&incoming);
        disk.set_intent_at(0, true, clicked_revision).unwrap();
        assert_eq!(disk.sequence, 10);
        assert_eq!(disk.settled_mute, 10);
        assert_eq!(disk.server.mute_revision, 2);
        assert!(!disk.server.muted);
        assert!(disk.pending_mute.is_none());
        // Even a delayed old cache cannot resurrect the superseded click.
        incoming.set_intent_at(0, true, clicked_revision).unwrap();
        disk.merge_disk(&incoming);
        assert!(disk.pending_mute.is_none());
        assert!(!disk.hides());
    }

    #[test]
    fn a_queued_stage_retains_the_revision_that_the_reader_actually_clicked() {
        let mut initial = stored();
        initial.server.mute_revision = 3;
        initial.server.block_revision = 8;
        let mut state = State { rows: BTreeMap::from([(2, initial)]), ..State::default() };
        state.stage(2, "two", 0, true).unwrap();
        let first = state.staged.get(&(2, 0)).unwrap().clone();
        assert_eq!(first.3, 3);
        assert_eq!(state.status, SyncStatus::Saving);
        assert!(state.filters.hides_user(&UserSummary {
            fid: 2,
            username: "two".into(),
            display_name: String::new(),
            pfp_url: None
        }));
        // A background observation arrives while the prior save owns the IDB
        // lock. Later persistence must still use revision3, not revision4.
        let row = state.rows.get_mut(&2).unwrap();
        row.server.mute_revision = 4;
        row.server.muted = false;
        row.set_intent_at(0, first.1, first.3).unwrap();
        assert!(row.pending_mute.is_none());
        assert_eq!(row.server.mute_revision, 4);
        assert_eq!(state.staged.get(&(2, 0)).unwrap().3, 3);
        state.stage(2, "two", 1, true).unwrap();
        let block = state.staged.get(&(2, 1)).unwrap();
        assert_eq!(block.3, 8);
        assert!(block.2 > first.2);
    }

    #[test]
    fn current_session_is_guarded_before_the_deferred_account_effect_runs() {
        let mut state = State::default();
        // Initial mounted state and an already signed-in cookie disagree:
        // a cached post must stay hidden even before account() is called.
        assert!(state.waiting_for(Some(1)));
        state.fid = Some(1);
        assert!(state.waiting_for(Some(1)));
        state.hydrated = true;
        state.disk_loaded = true;
        state.capability = Some(true);
        assert!(!state.waiting_for(Some(1)));
        // The user has already switched accounts, while the old effect and
        // its asynchronous operations still carry account1.
        assert!(state.waiting_for(Some(2)));
        assert!(state.waiting_for(None));
        // The public static copy has no signed-in identity or filters.
        assert!(!State::default().waiting_for(None));
    }

    #[test]
    fn explicit_revision_allocation_validates_kind_and_sequence_without_partial_mutation() {
        let mut row = stored();
        let original = row.clone();
        assert!(row.set_intent_at(2, true, 0).is_err());
        assert_eq!(row, original);
        row.sequence = u64::MAX;
        let original = row.clone();
        assert!(row.set_intent_at(0, true, 0).is_err());
        assert_eq!(row, original);
    }

    #[test]
    fn lost_ack_intent_cannot_resurrect_after_a_later_remote_undo() {
        let mut local = stored();
        local.set_intent(0, true).unwrap();
        let mut remote = stored();
        remote.server.mute_revision = 2;
        remote.server.muted = false;
        local.merge_disk(&remote);
        assert!(!local.hides());
        assert!(local.pending_mute.is_none());
        let mut stale = stored();
        stale.set_intent(0, true).unwrap();
        local.merge_disk(&stale);
        assert!(!local.hides());
        assert!(local.pending_mute.is_none());
    }

    fn ready_state() -> State {
        State {
            fid: Some(1),
            capability: Some(true),
            hydrated: true,
            disk_loaded: true,
            status: SyncStatus::Synced,
            ..State::default()
        }
    }

    #[test]
    fn visibility_ignores_sync_only_changes_on_an_unconfigured_empty_fixture() {
        let mut state = State { capability: Some(false), ..ready_state() };
        let visible = Visibility::of(&state, Some(1));
        assert!(!visible.waiting);
        assert!(visible.filters.is_empty());
        for status in [
            SyncStatus::Connecting,
            SyncStatus::Failed("Fixture endpoint".into()),
            SyncStatus::Saving,
            SyncStatus::Pending,
            SyncStatus::Synced,
        ] {
            state.status = status;
            state.paused = !state.paused;
            assert_eq!(Visibility::of(&state, Some(1)), visible);
        }
        // Capability discovery and a durable clear/outbox record do not
        // change which content is allowed after readiness is established.
        state.capability = Some(true);
        let mut cleared = stored();
        cleared.set_intent(0, false).unwrap();
        state.rows.insert(2, cleared);
        state.stage_sequence = 12;
        state.saving.insert((2, 0));
        state.rebuild();
        assert_eq!(Visibility::of(&state, Some(1)), visible);
        let row = state.rows.get_mut(&2).unwrap();
        let intent = row.pending_mute.clone().unwrap();
        row.acknowledge(0, &intent);
        state.saving.clear();
        state.rebuild();
        assert_eq!(Visibility::of(&state, Some(1)), visible);
    }

    #[test]
    fn visibility_outbox_commit_and_ack_do_not_remount_already_hidden_content() {
        let mut state = ready_state();
        let visible = Visibility::of(&state, Some(1));
        state.stage(2, "two", 0, true).unwrap();
        let hidden = Visibility::of(&state, Some(1));
        assert_ne!(hidden, visible);
        assert!(hidden.filters.hides_text("hello @two"));
        assert!(!hidden.blocked.contains(&2));
        let mut committed = stored();
        committed.server.target_username = "two".into();
        committed.set_intent(0, true).unwrap();
        state.rows.insert(2, committed);
        state.staged.clear();
        state.status = SyncStatus::Pending;
        state.rebuild();
        assert_eq!(Visibility::of(&state, Some(1)), hidden);
        let mut acknowledged = stored();
        acknowledged.server.target_username = "two".into();
        acknowledged.server.muted = true;
        acknowledged.server.mute_revision = 1;
        state.rows.get_mut(&2).unwrap().merge_disk(&acknowledged);
        state.status = SyncStatus::Synced;
        state.rebuild();
        assert!(state.rows[&2].pending_mute.is_none());
        assert_eq!(Visibility::of(&state, Some(1)), hidden);
    }

    #[test]
    fn visibility_pending_undo_never_reveals_before_authoritative_clear() {
        let mut row = stored();
        row.server.target_username = "two".into();
        row.server.blocked = true;
        row.server.block_revision = 1;
        let mut state = State { rows: BTreeMap::from([(2, row)]), ..ready_state() };
        state.rebuild();
        let hidden = Visibility::of(&state, Some(1));
        assert!(hidden.blocked.contains(&2));
        state.stage(2, "two", 1, false).unwrap();
        assert_eq!(Visibility::of(&state, Some(1)), hidden);
        state.rows.get_mut(&2).unwrap().set_intent(1, false).unwrap();
        state.staged.clear();
        state.rebuild();
        assert_eq!(Visibility::of(&state, Some(1)), hidden);
        let mut clear = stored();
        clear.server.target_username = "two".into();
        clear.server.block_revision = 2;
        state.rows.get_mut(&2).unwrap().merge_disk(&clear);
        state.rebuild();
        let revealed = Visibility::of(&state, Some(1));
        assert_ne!(revealed, hidden);
        assert!(!revealed.waiting);
        assert!(revealed.filters.is_empty());
        assert!(!revealed.blocked.contains(&2));
    }

    #[test]
    fn visibility_distinguishes_alias_filters_and_block_delivery_from_mutes() {
        let mut row = stored();
        row.server.target_username = "oldname".into();
        row.server.muted = true;
        let mut state = State { rows: BTreeMap::from([(2, row)]), ..ready_state() };
        state.rebuild();
        let muted = Visibility::of(&state, Some(1));
        assert!(muted.filters.hides_text("@oldname"));
        assert!(!muted.filters.hides_text("@newname"));
        state.rows.get_mut(&2).unwrap().aliases.insert("newname".into());
        state.rebuild();
        let renamed = Visibility::of(&state, Some(1));
        assert_ne!(renamed, muted);
        assert!(renamed.filters.hides_text("@newname"));
        // A block changes messaging even when an existing mute already hides
        // the same posts; clearing just that mute must not reveal anything.
        state.rows.get_mut(&2).unwrap().server.blocked = true;
        state.rebuild();
        let blocked = Visibility::of(&state, Some(1));
        assert_eq!(blocked.filters, renamed.filters);
        assert_ne!(blocked, renamed);
        assert!(blocked.blocked.contains(&2));
        state.rows.get_mut(&2).unwrap().server.muted = false;
        state.rebuild();
        assert_eq!(Visibility::of(&state, Some(1)), blocked);
        // A newly staged block also protects delivery before its disk write.
        let mut staged = ready_state();
        staged.stage(2, "two", 1, true).unwrap();
        assert!(Visibility::of(&staged, Some(1)).blocked.contains(&2));
    }

    #[test]
    fn visibility_keeps_disk_snapshot_and_account_switch_privacy_barriers() {
        assert!(!Visibility::of(&State::default(), None).waiting);
        assert!(Visibility::of(&State::default(), Some(1)).waiting);
        let mut state = State { capability: Some(false), disk_loaded: false, ..ready_state() };
        assert!(Visibility::of(&state, Some(1)).waiting);
        state.disk_loaded = true;
        let visible = Visibility::of(&state, Some(1));
        assert!(!visible.waiting);
        state.capability = Some(true);
        state.hydrated = false;
        assert_ne!(Visibility::of(&state, Some(1)), visible);
        assert!(Visibility::of(&state, Some(1)).waiting);
        state.hydrated = true;
        assert_eq!(Visibility::of(&state, Some(1)), visible);
        assert!(Visibility::of(&state, Some(2)).waiting);
        assert!(Visibility::of(&state, None).waiting);
        // An unconfigured server cannot bypass an already cached active
        // filter, even though empty legacy fixture state is permitted.
        let mut row = stored();
        row.server.muted = true;
        state.rows.insert(2, row);
        state.capability = Some(false);
        state.rebuild();
        assert!(Visibility::of(&state, Some(1)).waiting);
        assert!(!Visibility::of(&ready_state(), Some(1)).waiting);
    }
}

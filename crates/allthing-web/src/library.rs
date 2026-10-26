// SPDX-License-Identifier: AGPL-3.0-or-later
//! Private Library state, caller-scoped subscriptions and a committed offline outbox.
//! Shares the verified notification connection; never touches chat or public presence.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use allthing_core::library::{Account, CachedItem, Item, PendingBookmark, PendingRead, ReadIntent, canonical_hash};
use allthing_core::stdb::Called;
use js_sys::Array;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

use crate::library_disk;
use crate::stdb_bridge::{method, parse};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncStatus {
    SignedOut,
    Connecting,
    Ready,
    Offline,
    Failed(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivateKind {
    Seen,
    Bookmarks,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateCursor {
    pub generation: u64,
    pub after: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateEntry {
    pub hash: String,
    pub at: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivatePage {
    pub entries: Vec<PrivateEntry>,
    pub next: Option<PrivateCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingClear {
    operation: String,
    expected_generation: u64,
    sequence: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Metadata {
    pub(crate) fid: u64,
    server: Account,
    known: bool,
    pending_clear: Option<PendingClear>,
    sequence: u64,
    clear_settled: u64,
    legacy_imported: bool,
}
impl Metadata {
    pub(crate) fn merge_disk(&mut self, incoming: &Self) {
        if self.fid != incoming.fid {
            return;
        }
        if incoming.known
            && (!self.known
                || (incoming.server.generation, incoming.server.revision)
                    >= (self.server.generation, self.server.revision))
        {
            self.server = incoming.server.clone();
            self.known = true;
        }
        self.sequence = self.sequence.max(incoming.sequence);
        self.clear_settled = self.clear_settled.max(incoming.clear_settled);
        self.legacy_imported |= incoming.legacy_imported;
        self.pending_clear = self
            .pending_clear
            .iter()
            .chain(incoming.pending_clear.iter())
            .filter(|p| p.sequence > self.clear_settled)
            .max_by_key(|p| p.sequence)
            .cloned();
        if self.pending_clear.as_ref().is_some_and(|p| self.known && self.server.generation > p.expected_generation)
            && let Some(p) = self.pending_clear.take()
        {
            self.clear_settled = self.clear_settled.max(p.sequence);
        }
    }
    fn effective(&self) -> Account {
        let mut account = self.server.clone();
        account.fid = self.fid;
        if let Some(clear) = &self.pending_clear {
            account.generation = clear.expected_generation.saturating_add(1);
        }
        account
    }
}

#[derive(Clone)]
struct State {
    fid: Option<u64>,
    capability: Option<bool>,
    status: SyncStatus,
    revision: u64,
    metadata: Metadata,
    items: BTreeMap<String, CachedItem>,
    checked: BTreeSet<String>,
    loaded: BTreeSet<String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            fid: None,
            capability: None,
            status: SyncStatus::SignedOut,
            revision: 0,
            metadata: Metadata::default(),
            items: BTreeMap::new(),
            checked: BTreeSet::new(),
            loaded: BTreeSet::new(),
        }
    }
}
#[derive(Clone, Copy)]
pub struct Library(RwSignal<State>, crate::components::signin::Session);
thread_local! {
    static HANDLE: Cell<Option<Library>> = const { Cell::new(None) };
    static CURRENT: RefCell<Option<Broker>> = const { RefCell::new(None) };
    static WATCHES: RefCell<BTreeMap<String, BTreeSet<String>>> = const { RefCell::new(BTreeMap::new()) };
    static IDS: Cell<u64> = const { Cell::new(0) };
}
pub fn use_library() -> Library {
    use_context::<Library>().expect("Library context")
}
fn handle() -> Option<Library> {
    HANDLE.with(Cell::get)
}
fn next_id() -> u64 {
    IDS.with(|id| {
        let n = id.get().wrapping_add(1);
        id.set(n);
        n
    })
}
fn canonical(hash: &str) -> Result<String, String> {
    canonical_hash(hash).ok_or_else(|| "Invalid post hash".into())
}
fn now() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}
fn all_watches() -> BTreeSet<String> {
    WATCHES.with(|all| all.borrow().values().flat_map(|v| v.iter().cloned()).collect())
}

impl Library {
    pub fn new() -> Self {
        let session = crate::components::signin::use_session();
        let fid = session.user.with_untracked(|u| u.as_ref().map(|u| u.fid).filter(|fid| *fid > 0));
        let state = State {
            fid,
            status: if fid.is_some() { SyncStatus::Connecting } else { SyncStatus::SignedOut },
            metadata: Metadata { fid: fid.unwrap_or(0), ..Metadata::default() },
            ..State::default()
        };
        let library = Self(RwSignal::new(state), session);
        HANDLE.with(|h| h.set(Some(library)));
        library
    }
    fn fid(self) -> Option<u64> {
        self.1.user.with(|u| u.as_ref().map(|u| u.fid).filter(|fid| *fid > 0))
    }
    fn same(self) -> bool {
        let fid = self.fid();
        self.0.with(|s| s.fid == fid)
    }
    pub fn status(self) -> SyncStatus {
        if !self.same() {
            return if self.fid().is_some() { SyncStatus::Connecting } else { SyncStatus::SignedOut };
        }
        self.0.with(|s| s.status.clone())
    }
    pub fn revision(self) -> u64 {
        self.0.with(|s| s.revision)
    }
    pub fn generation(self) -> u64 {
        self.0.with(|s| s.metadata.effective().generation)
    }
    pub fn remote_required(self) -> bool {
        let fid = self.fid();
        fid.is_some() && self.0.with(|s| s.fid != fid || s.capability != Some(false))
    }
    pub fn membership_ready(self, hashes: &[String]) -> bool {
        if !self.same() {
            return false;
        }
        self.0.with(|s| {
            s.capability == Some(false)
                || (s.status == SyncStatus::Ready
                    && s.metadata.legacy_imported
                    && hashes.iter().all(|h| s.checked.contains(h) && s.loaded.contains(h)))
        })
    }
    pub fn seen(self, hash: &str) -> Option<bool> {
        if !self.same() {
            return None;
        }
        self.0.with(|s| {
            s.items.get(hash).filter(|c| c.known || c.read.is_some()).map(|c| c.seen(s.metadata.effective().generation))
        })
    }
    pub fn bookmarked(self, hash: &str) -> Option<bool> {
        if !self.same() {
            return None;
        }
        self.0.with(|s| s.items.get(hash).and_then(CachedItem::bookmarked))
    }
    pub fn watch(self, hash: &str) {
        let key = format!("card-{}", next_id());
        let cleanup = key.clone();
        // The constructing card owns this registration. Register retirement
        // before any signal writes; subscription plumbing must not become a
        // dependency of the parent render/memo or of its cleanup callback.
        on_cleanup(move || untrack(|| self.unwatch_batch(&cleanup)));
        untrack(|| self.watch_batch(&key, &[hash.into()]));
    }
    pub fn watch_batch(self, owner: &str, hashes: &[String]) {
        let values = hashes.iter().filter_map(|h| canonical_hash(h)).collect::<BTreeSet<_>>();
        WATCHES.with(|w| {
            w.borrow_mut().insert(owner.into(), values);
        });
        if let Some(b) = current() {
            b.load_watched();
            b.watches();
        }
        crate::notification_sync::wake_urgent();
    }
    pub fn unwatch_batch(self, owner: &str) {
        WATCHES.with(|w| {
            w.borrow_mut().remove(owner);
        });
        if let Some(b) = current() {
            b.watches();
        }
    }
    pub fn retry(self) {
        crate::notification_sync::wake_urgent();
        if let Some(b) = current() {
            b.inner.disk_failed.set(false);
            b.bootstrap();
            b.persist();
            b.load_watched();
            b.watches();
            b.drain();
        }
    }
    pub fn toggle_bookmark(self, hash: &str) -> Result<(), String> {
        let hash = canonical(hash)?;
        let b = require()?;
        self.bookmarked(&hash).ok_or("Wait for this post's saved state to load")?;
        b.action(hash, Action::Toggle)
    }
    pub fn record_seen(self, hash: &str, at: i64) -> Result<(), String> {
        let hash = canonical(hash)?;
        let b = require()?;
        if self.seen(&hash) == Some(true) {
            return Ok(());
        }
        b.action(hash, Action::Seen(at.min(now()).max(1)))
    }
    pub fn forget_seen(self, hash: &str) -> Result<(), String> {
        let hash = canonical(hash)?;
        require()?.action(hash, Action::Unread)
    }
    pub fn clear_seen(self) -> Result<(), String> {
        let b = require()?;
        if !self.0.with_untracked(|s| s.metadata.known) {
            return Err("Wait for Library state to load before clearing history".into());
        }
        if self.0.with_untracked(|s| s.metadata.pending_clear.is_some()) {
            return Ok(());
        }
        b.clear_action()?;
        Ok(())
    }
    pub async fn page(self, kind: PrivateKind, cursor: Option<PrivateCursor>) -> Result<PrivatePage, String> {
        let b = require()?;
        b.page(kind, cursor).await
    }
}
fn empty(fid: u64, hash: &str, generation: u64) -> CachedItem {
    CachedItem { server: Item { fid, hash: hash.into(), generation, ..Item::default() }, ..CachedItem::default() }
}
fn current() -> Option<Broker> {
    CURRENT.with(|c| c.borrow().clone())
}
fn require() -> Result<Broker, String> {
    let b = current().filter(|b| b.active()).ok_or("Sign in to use your Library")?;
    if b.library.0.with_untracked(|s| s.capability == Some(false)) {
        return Err("Private Library sync is not configured on this server".into());
    }
    Ok(b)
}

#[derive(Clone)]
struct Broker {
    fid: u64,
    library: Library,
    inner: Rc<Inner>,
}
struct Inner {
    stopped: Cell<bool>,
    attempt: Cell<u64>,
    bridge: RefCell<Option<JsValue>>,
    base: Cell<bool>,
    disk: Cell<bool>,
    disk_failed: Cell<bool>,
    bootstrapping: Cell<bool>,
    clear_busy: Cell<bool>,
    loading: RefCell<BTreeSet<String>>,
    groups: RefCell<BTreeMap<String, Lease>>,
    writing: Cell<bool>,
    drain_again: Cell<bool>,
    persisting: Cell<bool>,
    writes: RefCell<VecDeque<Write>>,
    in_flight: RefCell<BTreeMap<String, Flight>>,
    buffered: RefCell<BTreeMap<String, Item>>,
    busy: RefCell<BTreeSet<(String, bool)>>,
    touches: RefCell<BTreeMap<String, u64>>,
    clock: Cell<u64>,
}
#[derive(Clone)]
enum Write {
    Item(CachedItem),
    Metadata(Metadata),
}
#[derive(Clone)]
enum Action {
    Toggle,
    Seen(i64),
    Unread,
}
impl Action {
    fn bookmark(&self) -> bool {
        matches!(self, Self::Toggle)
    }
    fn pending(&self, item: &CachedItem) -> bool {
        if self.bookmark() { item.bookmark.is_some() } else { item.read.is_some() }
    }
}
#[derive(Clone)]
enum Flight {
    Read(PendingRead),
    Bookmark(PendingBookmark),
}
struct Lease {
    bridge: JsValue,
    tag: String,
    hashes: BTreeSet<String>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(f) = method(&self.bridge, "unwatch") {
            let _ = f.call1(&self.bridge, &self.tag.clone().into());
        }
    }
}
const MAX_MEMORY: usize = 2000;
fn evictions(
    items: &BTreeMap<String, CachedItem>,
    protected: &BTreeSet<String>,
    touches: &BTreeMap<String, u64>,
) -> Vec<String> {
    let mut eligible: Vec<_> = items
        .iter()
        .filter(|(hash, item)| !protected.contains(*hash) && !item.pending())
        .map(|(hash, _)| (touches.get(hash).copied().unwrap_or(0), hash.clone()))
        .collect();
    eligible.sort();
    eligible.into_iter().take(items.len().saturating_sub(MAX_MEMORY)).map(|(_, hash)| hash).collect()
}
impl Broker {
    fn active(&self) -> bool {
        !self.inner.stopped.get()
            && self.library.1.user.with_untracked(|u| u.as_ref().map(|u| u.fid)) == Some(self.fid)
            && current().is_some_and(|b| Rc::ptr_eq(&b.inner, &self.inner))
    }
    fn touch(&self, hash: &str) {
        let next = self.inner.clock.get().wrapping_add(1);
        self.inner.clock.set(next);
        self.inner.touches.borrow_mut().insert(hash.into(), next);
    }
    fn prune(&self) {
        if !self.active() {
            return;
        }
        let mut protected = all_watches();
        protected.extend(self.inner.busy.borrow().iter().map(|(hash, _)| hash.clone()));
        protected.extend(self.inner.in_flight.borrow().keys().cloned());
        protected.extend(self.inner.loading.borrow().iter().cloned());
        self.library.0.update(|s| {
            for hash in evictions(&s.items, &protected, &self.inner.touches.borrow()) {
                s.items.remove(&hash);
                s.loaded.remove(&hash);
                s.checked.remove(&hash);
            }
            s.loaded.retain(|hash| protected.contains(hash) || s.items.contains_key(hash));
            self.inner.touches.borrow_mut().retain(|hash, _| s.items.contains_key(hash));
        });
    }
    fn check(&self, attempt: u64) -> Result<(), String> {
        if self.active() && self.inner.attempt.get() == attempt && self.inner.base.get() {
            Ok(())
        } else {
            Err("Library session changed or disconnected".into())
        }
    }
    fn failed(&self, error: String) {
        if self.active() {
            self.inner.groups.borrow_mut().clear();
            self.library.0.update(|s| {
                s.status = SyncStatus::Failed(error);
                s.checked.clear();
            });
        }
    }
    fn sequence(&self) -> u64 {
        let random = crate::keys::random::<4>().map(u32::from_le_bytes).unwrap_or(0) as u64 % 1_000_000;
        let base = (js_sys::Date::now().max(0.0) as u64).saturating_mul(1_000_000).saturating_add(random);
        self.library.0.update(|s| s.metadata.sequence = base.max(s.metadata.sequence.saturating_add(1)));
        self.library.0.with_untracked(|s| s.metadata.sequence)
    }
    fn stop(&self) {
        self.inner.stopped.set(true);
        self.inner.base.set(false);
        self.inner.groups.borrow_mut().clear();
        self.inner.bridge.borrow_mut().take();
    }
    fn persist_item(&self, hash: &str) {
        if let Some(item) = self.library.0.with_untracked(|s| s.items.get(hash).cloned()) {
            self.inner.writes.borrow_mut().push_back(Write::Item(item));
            self.persist();
        }
    }
    fn persist_meta(&self) {
        let value = self.library.0.with_untracked(|s| s.metadata.clone());
        self.inner.writes.borrow_mut().push_back(Write::Metadata(value));
        self.persist();
    }
    fn persist(&self) {
        if !self.active() || self.inner.persisting.replace(true) {
            return;
        }
        let b = self.clone();
        leptos::task::spawn_local(async move {
            loop {
                let Some(write) = b.inner.writes.borrow_mut().pop_front() else { break };
                let answer = match &write {
                    Write::Item(item) => library_disk::keep_item(b.fid, item).await,
                    Write::Metadata(meta) => match serde_json::to_string(meta) {
                        Ok(json) => library_disk::keep_account(b.fid, &json).await,
                        Err(e) => Err(e.to_string()),
                    },
                };
                // Finish every already accepted disk write under its captured
                // FID even after sign-out; never send it over an old socket.
                if let Err(e) = answer {
                    b.inner.writes.borrow_mut().push_front(write);
                    b.inner.disk_failed.set(true);
                    b.failed(format!("Your Library change could not be saved on this device: {e}"));
                    break;
                }
                if matches!(&write, Write::Metadata(_)) && b.active() {
                    match library_disk::account(b.fid).await {
                        Ok(Some(json)) => match serde_json::from_str(&json) {
                            Ok(meta) => b.merge_metadata(meta),
                            Err(_) => {
                                b.inner.disk_failed.set(true);
                                b.failed("Unreadable Library metadata".into());
                                break;
                            }
                        },
                        Ok(None) => {
                            b.inner.disk_failed.set(true);
                            b.failed("Library metadata was not committed".into());
                            break;
                        }
                        Err(error) => {
                            b.inner.disk_failed.set(true);
                            b.failed(error);
                            break;
                        }
                    }
                }
            }
            b.inner.persisting.set(false);
            if b.active() {
                b.drain();
            }
        });
    }
    fn load_watched(&self) {
        let hashes: Vec<_> = all_watches()
            .into_iter()
            .filter(|h| {
                !self.library.0.with_untracked(|s| s.loaded.contains(h))
                    && self.inner.loading.borrow_mut().insert(h.clone())
            })
            .collect();
        if hashes.is_empty() {
            return;
        }
        let b = self.clone();
        leptos::task::spawn_local(async move {
            let rows = library_disk::items(b.fid, &hashes).await;
            if !b.active() {
                return;
            }
            for hash in &hashes {
                b.inner.loading.borrow_mut().remove(hash);
            }
            match rows {
                Ok(rows) => {
                    b.merge_cached(rows);
                    b.library.0.update(|s| s.loaded.extend(hashes));
                }
                Err(e) => {
                    b.inner.disk_failed.set(true);
                    b.failed(e);
                }
            }
        });
    }
    fn merge_cached(&self, rows: Vec<CachedItem>) {
        self.library.0.update(|s| {
            for row in rows {
                let hash = row.server.hash.clone();
                self.touch(&hash);
                let flight = self.inner.in_flight.borrow().get(&hash).cloned();
                let (read, bookmark) = match flight {
                    Some(Flight::Read(sent)) => (Some(sent.sequence), None),
                    Some(Flight::Bookmark(sent)) => (None, Some(sent.sequence)),
                    None => (None, None),
                };
                let c = s
                    .items
                    .entry(hash)
                    .or_insert_with(|| empty(self.fid, &row.server.hash, s.metadata.effective().generation));
                let old = c.clone();
                c.merge_disk_preserving_inflight(&row, read, bookmark);
                if old != *c {
                    s.revision += 1;
                }
            }
        });
        self.prune();
    }
    fn bridge(&self) -> Result<JsValue, String> {
        self.inner.bridge.borrow().clone().ok_or_else(|| "Library is disconnected".into())
    }
    fn raw(&self, name: &str, args: &[JsValue]) -> Result<JsValue, String> {
        self.check(self.inner.attempt.get())?;
        let bridge = self.bridge()?;
        method(&bridge, name)?.apply(&bridge, &args.iter().collect::<Array>()).map_err(crate::stdb_bridge::js_error)
    }
    async fn call(&self, name: &str, args: &[JsValue]) -> Result<(), String> {
        let attempt = self.inner.attempt.get();
        let result: Called = parse(crate::notification_sync::bounded(self.raw(name, args)?).await?)?;
        self.check(attempt)?;
        result.result()
    }
    async fn snapshot(&self, accessor: &str, query: String) -> Result<Vec<Item>, String> {
        let attempt = self.inner.attempt.get();
        let bridge = self.bridge()?;
        let tag = format!("library-once-{}", next_id());
        let _lease = Lease { bridge, tag: tag.clone(), hashes: BTreeSet::new() };
        let json = serde_json::to_string(&[query]).map_err(|e| e.to_string())?;
        let value = self.raw("snapshot", &[tag.into(), accessor.into(), json.into()])?;
        let rows = parse(crate::notification_sync::bounded(value).await?)?;
        self.check(attempt)?;
        Ok(rows)
    }
    async fn membership(&self, hashes: &[String]) -> Result<Vec<Item>, String> {
        if hashes.is_empty() || hashes.len() > 100 {
            return Err("Invalid Library membership batch".into());
        }
        let rows = self
            .snapshot("myLibraryMembership", format!("SELECT * FROM my_library_membership('{}')", hashes.join(",")))
            .await?;
        self.validate_membership(hashes, rows)
    }
    fn validate_membership(&self, hashes: &[String], rows: Vec<Item>) -> Result<Vec<Item>, String> {
        let generation = self.library.0.with_untracked(|s| s.metadata.server.generation);
        let wanted: BTreeSet<_> = hashes.iter().cloned().collect();
        let rows = fold_rows(rows, self.fid, generation)
            .into_iter()
            .filter(|(hash, _)| wanted.contains(hash))
            .collect::<BTreeMap<_, _>>();
        if rows.len() != wanted.len() {
            return Err("Library membership is incomplete; try again".into());
        }
        Ok(hashes.iter().filter_map(|h| rows.get(h).cloned()).collect())
    }
    fn observe(&self, row: Item) {
        if self.inner.in_flight.borrow().contains_key(&row.hash) {
            let account = self.library.0.with_untracked(|s| s.metadata.effective());
            let mut buffered = self.inner.buffered.borrow_mut();
            if let Some(old) = buffered.get_mut(&row.hash) {
                merge_row(old, row, &account);
            } else {
                buffered.insert(row.hash.clone(), row);
            }
            return;
        }
        let hash = row.hash.clone();
        self.touch(&hash);
        let mut changed = false;
        self.library.0.update(|s| {
            let account = s.metadata.effective();
            let c = s.items.entry(hash.clone()).or_insert_with(|| empty(self.fid, &hash, account.generation));
            let old = c.clone();
            c.observe(row, &account);
            changed = old != *c;
            if changed {
                s.revision += 1;
            }
        });
        if changed {
            self.persist_item(&hash);
        }
        self.prune();
    }
    fn watches(&self) {
        self.prune();
        if !self.active()
            || !self.inner.base.get()
            || !self.inner.disk.get()
            || !self.library.0.with_untracked(|s| s.metadata.known && s.metadata.pending_clear.is_none())
        {
            return;
        }
        let hashes: Vec<_> = all_watches().into_iter().collect();
        let groups: BTreeMap<_, _> =
            hashes.chunks(100).map(|hs| (hs.join(","), hs.iter().cloned().collect::<BTreeSet<_>>())).collect();
        self.inner.groups.borrow_mut().retain(|signature, lease| {
            if groups.contains_key(signature) {
                true
            } else {
                self.library.0.update(|s| {
                    for hash in &lease.hashes {
                        s.checked.remove(hash);
                    }
                });
                false
            }
        });
        for (signature, hashes) in groups {
            if self.inner.groups.borrow().contains_key(&signature) {
                continue;
            }
            let Ok(bridge) = self.bridge() else { return };
            let tag = format!("library-live-{}", next_id());
            let query = serde_json::to_string(&[format!("SELECT * FROM my_library_membership('{signature}')")])
                .unwrap_or_default();
            let Ok(value) = self.raw("watch", &[tag.clone().into(), query.into()]) else { continue };
            self.inner
                .groups
                .borrow_mut()
                .insert(signature.clone(), Lease { bridge, tag: tag.clone(), hashes: hashes.clone() });
            let b = self.clone();
            let attempt = self.inner.attempt.get();
            leptos::task::spawn_local(async move {
                let result = crate::notification_sync::bounded(value).await;
                if b.check(attempt).is_err()
                    || !b.inner.groups.borrow().get(&signature).is_some_and(|lease| lease.tag == tag)
                {
                    return;
                }
                let answer = result.and_then(|_| {
                    let bridge = b.bridge()?;
                    let value = method(&bridge, "rows")?
                        .call1(&bridge, &"myLibraryMembership".into())
                        .map_err(crate::stdb_bridge::js_error)?;
                    b.validate_membership(&hashes.iter().cloned().collect::<Vec<_>>(), parse(value)?)
                });
                match answer {
                    Ok(rows) => {
                        for row in rows {
                            b.observe(row);
                        }
                        b.library.0.update(|s| {
                            s.checked.extend(hashes);
                        });
                        b.drain();
                    }
                    Err(e) => {
                        b.inner.groups.borrow_mut().remove(&signature);
                        b.failed(e);
                    }
                }
            });
        }
    }
    async fn page(&self, kind: PrivateKind, cursor: Option<PrivateCursor>) -> Result<PrivatePage, String> {
        if self.library.status() != SyncStatus::Ready {
            return Err("Connect your Library to load this page; saved offline changes are kept".into());
        }
        let generation = self.library.0.with_untracked(|s| s.metadata.server.generation);
        if self.library.0.with_untracked(|s| s.metadata.pending_clear.is_some()) {
            return Err("Your history clear is still syncing".into());
        }
        let after = match cursor {
            Some(c) if c.generation != generation => return Err("History changed; refresh this list".into()),
            Some(c) => c.after,
            None => String::new(),
        };
        if !valid_cursor(&after) {
            return Err("Invalid Library cursor".into());
        }
        let rows = self
            .snapshot(
                "myLibraryPage",
                format!(
                    "SELECT * FROM my_library_page({},{generation},'{after}',25)",
                    if kind == PrivateKind::Seen { 0 } else { 1 }
                ),
            )
            .await?;
        if self.library.generation() != generation {
            return Err("History changed; refresh this list".into());
        }
        let account = self.library.0.with_untracked(|s| s.metadata.effective());
        let mut folded = fold_rows(rows, self.fid, generation);
        self.library.0.with_untracked(|s| {
            for (hash, row) in &mut folded {
                if let Some(cache) = s.items.get(hash).filter(|c| c.known) {
                    merge_row(row, cache.server.clone(), &account);
                }
            }
        });
        let mut rows: Vec<_> = folded
            .into_values()
            .filter(|r| match kind {
                PrivateKind::Seen => r.seen,
                PrivateKind::Bookmarks => r.bookmarked,
            })
            .filter(|r| after.is_empty() || before(r, &after, kind))
            .collect();
        rows.sort_by(|a, b| stamp(b, kind).cmp(&stamp(a, kind)).then_with(|| b.hash.cmp(&a.hash)));
        rows.dedup_by(|a, b| a.hash == b.hash);
        let more = rows.len() > 25;
        rows.truncate(25);
        let entries: Vec<_> = rows.iter().map(|r| PrivateEntry { hash: r.hash.clone(), at: stamp(r, kind) }).collect();
        for row in rows {
            self.observe(row);
        }
        let next = if more {
            entries.last().map(|e| PrivateCursor { generation, after: format!("{}:{}", e.at, e.hash) })
        } else {
            None
        };
        Ok(PrivatePage { entries, next })
    }
    fn merge_metadata(&self, meta: Metadata) {
        if meta.fid != self.fid {
            return;
        }
        if meta.known {
            observe_account(self, meta.server.clone());
        }
        self.library.0.update(|s| {
            let old = s.metadata.clone();
            s.metadata.merge_disk(&meta);
            let account = s.metadata.effective();
            let mut changed = old != s.metadata;
            for (hash, c) in &mut s.items {
                if !self.inner.in_flight.borrow().contains_key(hash) {
                    let before = c.clone();
                    c.reconcile(&account);
                    changed |= *c != before;
                }
            }
            if changed {
                s.revision += 1;
            }
        });
    }
    fn clear_action(&self) -> Result<(), String> {
        if self.inner.clear_busy.replace(true) {
            return Err("A history clear is already being saved".into());
        }
        let b = self.clone();
        leptos::task::spawn_local(async move {
            let result = async {
                let kept = library_disk::account(b.fid).await?;
                if !b.active() {
                    return Err("Library session changed".into());
                }
                if let Some(json) = kept {
                    let meta = serde_json::from_str(&json).map_err(|_| "Unreadable Library metadata")?;
                    b.merge_metadata(meta);
                }
                if b.library.0.with_untracked(|s| s.metadata.pending_clear.is_some()) {
                    return Err("A history clear from this account is still syncing".into());
                }
                let sequence = b.sequence();
                let random = crate::keys::random::<16>().ok_or("Couldn't create a clear operation")?;
                let operation = random.iter().map(|b| format!("{b:02x}")).collect();
                let mut changed = Vec::new();
                b.library.0.update(|s| {
                    s.metadata.pending_clear =
                        Some(PendingClear { operation, expected_generation: s.metadata.server.generation, sequence });
                    let account = s.metadata.effective();
                    for (hash, c) in &mut s.items {
                        let old = c.clone();
                        c.reconcile(&account);
                        if old != *c {
                            changed.push(hash.clone());
                        }
                    }
                    s.checked.clear();
                    s.revision += 1;
                });
                b.inner.groups.borrow_mut().clear();
                b.persist_meta();
                for hash in changed {
                    b.persist_item(&hash);
                }
                Ok::<_, String>(())
            }
            .await;
            b.inner.clear_busy.set(false);
            if b.active()
                && let Err(error) = result
            {
                b.failed(error);
            }
        });
        Ok(())
    }
    fn bootstrap(&self) {
        if self.inner.disk.get() || self.inner.bootstrapping.replace(true) {
            return;
        }
        let fid = self.fid;
        let b = self.clone();
        leptos::task::spawn_local(async move {
            let result = async {
                let metadata = library_disk::account(fid).await?;
                let pending = library_disk::pending(fid, 100).await?;
                Ok::<_, String>((metadata, pending))
            }
            .await;
            b.inner.bootstrapping.set(false);
            if !b.active() {
                return;
            }
            match result {
                Ok((metadata, pending)) => {
                    if let Some(json) = metadata {
                        match serde_json::from_str::<Metadata>(&json) {
                            Ok(meta) => b.merge_metadata(meta),
                            Err(_) => {
                                b.inner.disk_failed.set(true);
                                b.failed("Unreadable Library account metadata".into());
                                return;
                            }
                        }
                    }
                    b.merge_cached(pending);
                    b.inner.disk.set(true);
                    b.ready();
                    b.watches();
                    b.drain();
                }
                Err(e) => {
                    b.inner.disk_failed.set(true);
                    b.failed(e);
                }
            }
        });
    }
    fn action(&self, hash: String, action: Action) -> Result<(), String> {
        let pending = self.library.0.with_untracked(|s| s.items.get(&hash).is_some_and(|item| action.pending(item)));
        let inflight = self.inner.in_flight.borrow().get(&hash).is_some_and(|flight| {
            matches!((action.bookmark(), flight), (true, Flight::Bookmark(_)) | (false, Flight::Read(_)))
        });
        let busy = (hash.clone(), action.bookmark());
        if pending || inflight || !self.inner.busy.borrow_mut().insert(busy.clone()) {
            if matches!(action, Action::Seen(_)) {
                return Ok(());
            }
            return Err("Syncing the previous change to this post; try again when it finishes".into());
        }
        let b = self.clone();
        leptos::task::spawn_local(async move {
            let answer = async {
                let old = library_disk::items(b.fid, std::slice::from_ref(&hash)).await?;
                if !b.active() {
                    return Err("Library session changed".into());
                }
                b.merge_cached(old);
                if b.library.0.with_untracked(|s| s.items.get(&hash).is_some_and(|item| action.pending(item))) {
                    if matches!(action, Action::Seen(_)) {
                        return Ok(());
                    }
                    return Err("Another tab is syncing this post's previous change; try again when it finishes".into());
                }
                let sequence = b.sequence();
                let mut result: Result<(), String> = Ok(());
                b.library.0.update(|s| {
                    let generation = s.metadata.effective().generation;
                    let c = s.items.entry(hash.clone()).or_insert_with(|| empty(b.fid, &hash, generation));
                    match action {
                        Action::Toggle => match c.bookmarked() {
                            Some(value) => {
                                c.bookmark = Some(PendingBookmark {
                                    desired: !value,
                                    expected_revision: c.server.bookmark_revision,
                                    sequence,
                                })
                            }
                            None => result = Err("Wait for this post's saved state to load".into()),
                        },
                        Action::Seen(at) => {
                            if !c.seen(generation) {
                                c.read = Some(PendingRead {
                                    generation,
                                    expected_revision: c.server.seen_revision,
                                    intent: ReadIntent::Seen { at },
                                    sequence,
                                });
                            }
                        }
                        Action::Unread => {
                            c.read = Some(PendingRead {
                                generation,
                                expected_revision: c.server.seen_revision,
                                intent: ReadIntent::Unread,
                                sequence,
                            })
                        }
                    }
                    s.revision += 1;
                });
                result?;
                b.persist_item(&hash);
                Ok::<_, String>(())
            }
            .await;
            b.inner.busy.borrow_mut().remove(&busy);
            if b.active()
                && let Err(error) = answer
            {
                b.failed(error);
            }
        });
        Ok(())
    }
    fn ready(&self) {
        if self.inner.base.get()
            && self.inner.disk.get()
            && !self.inner.disk_failed.get()
            && self.library.0.with_untracked(|s| s.metadata.legacy_imported)
        {
            let restore = self.library.0.with_untracked(|s| s.status != SyncStatus::Ready);
            self.library.0.update(|s| {
                s.status = SyncStatus::Ready;
            });
            // A failed operation retires all managed subscriptions. A later
            // successful drain must restore them before presenting Ready.
            // Rebuild only after releasing the State/RefCell borrows above.
            if restore {
                self.watches();
            }
        }
    }
    fn yield_for_clear(&self) -> bool {
        let waiting = self.library.0.with_untracked(|s| s.metadata.pending_clear.is_some());
        if waiting {
            self.inner.drain_again.set(true);
        }
        waiting
    }
    fn drain(&self) {
        if !self.active()
            || !self.inner.base.get()
            || !self.inner.disk.get()
            || self.inner.disk_failed.get()
            || self.inner.persisting.get()
            || !self.inner.writes.borrow().is_empty()
        {
            return;
        }
        if self.inner.writing.replace(true) {
            self.inner.drain_again.set(true);
            return;
        }
        let b = self.clone();
        let attempt = self.inner.attempt.get();
        leptos::task::spawn_local(async move {
            let result = b.drain_all(attempt).await;
            b.inner.writing.set(false);
            if b.active() && b.inner.attempt.get() == attempt {
                match result {
                    Err(error) => b.failed(error),
                    Ok(()) => {
                        if b.inner.drain_again.replace(false) {
                            b.drain();
                        }
                    }
                }
            } else if b.active() {
                b.drain();
            }
        });
    }
    async fn drain_all(&self, attempt: u64) -> Result<(), String> {
        self.check(attempt)?;
        // A clear is a durable, idempotent generation change and precedes reads
        // created after it. The module's operation ID makes a lost ACK safe.
        let clear = self.library.0.with_untracked(|s| s.metadata.pending_clear.clone());
        if let Some(clear) = clear {
            let sent = self
                .call("clearSeen", &[(clear.expected_generation as f64).into(), clear.operation.clone().into()])
                .await;
            self.check(attempt)?;
            self.refresh_account().await?;
            self.check(attempt)?;
            if self.library.0.with_untracked(|s| s.metadata.pending_clear.is_some()) {
                sent?;
            }
            if self.library.0.with_untracked(|s| s.metadata.pending_clear.is_some()) {
                return Err("History clear has not been acknowledged; retry to reconnect".into());
            }
        }
        self.migrate_legacy(attempt).await?;
        self.ready();
        loop {
            self.check(attempt)?;
            if self.yield_for_clear() {
                break;
            }
            if self.inner.persisting.get() || !self.inner.writes.borrow().is_empty() {
                break;
            }
            let pending = library_disk::pending(self.fid, 100).await?;
            self.check(attempt)?;
            if pending.is_empty() {
                break;
            }
            // Point membership is authoritative before replay. A higher remote
            // tombstone cancels offline intent; never rebase an old conflict.
            let hashes: Vec<_> = pending.iter().map(|p| p.server.hash.clone()).collect();
            self.merge_cached(pending);
            let rows = self.membership(&hashes).await?;
            for row in rows {
                let hash = row.hash.clone();
                self.library.0.update(|s| {
                    let account = s.metadata.effective();
                    let c = s.items.entry(hash.clone()).or_insert_with(|| empty(self.fid, &hash, account.generation));
                    c.observe(row, &account);
                    clamp_replay(c, now(), account.cleared_at);
                    s.revision += 1;
                });
            }
            let mut candidates = Vec::new();
            for hash in &hashes {
                let item = self
                    .library
                    .0
                    .with_untracked(|s| s.items.get(hash).cloned())
                    .ok_or("Missing Library outbox item")?;
                library_disk::keep_item(self.fid, &item).await?;
                self.check(attempt)?;
                // Read back the atomically merged row: another tab can have
                // committed newer intent while this snapshot was being saved.
                let durable = library_disk::items(self.fid, std::slice::from_ref(hash)).await?;
                self.check(attempt)?;
                self.merge_cached(durable);
                if let Some(item) =
                    self.library.0.with_untracked(|s| s.items.get(hash).cloned()).filter(CachedItem::pending)
                {
                    candidates.push(item);
                }
            }
            if candidates.is_empty() {
                continue;
            }
            // A clear may have been requested while the point reads/commits
            // above were awaiting. Restart with that durable generation change
            // before dispatching any intent created after it.
            if self.yield_for_clear() {
                break;
            }
            // One reducer flight at a time. Automatic Seen observations share
            // a bounded batch; user removals/bookmarks are serialized separately.
            let seen: Vec<_> = candidates
                .iter()
                .filter_map(|c| {
                    c.read
                        .as_ref()
                        .filter(|r| matches!(r.intent, ReadIntent::Seen { .. }))
                        .map(|r| (c.server.hash.clone(), r.clone()))
                })
                .collect();
            if !seen.is_empty() {
                let generation = seen[0].1.generation;
                let seen: Vec<_> = seen.into_iter().filter(|(_, r)| r.generation == generation).collect();
                for (hash, read) in &seen {
                    self.inner.in_flight.borrow_mut().insert(hash.clone(), Flight::Read(read.clone()));
                }
                let entries:Vec<_>=seen.iter().map(|(hash,r)|serde_json::json!({"hash":hash,"at":match r.intent {ReadIntent::Seen{at}=>at.min(now()),ReadIntent::Unread=>0},"expected_revision":r.expected_revision})).collect();
                let result = self
                    .call(
                        "markSeenBatch",
                        &[
                            (generation as f64).into(),
                            serde_json::to_string(&entries).map_err(|e| e.to_string())?.into(),
                        ],
                    )
                    .await;
                self.finish_flights(&seen.iter().map(|(h, _)| h.clone()).collect::<Vec<_>>(), result.is_ok(), attempt)
                    .await?;
                result?;
            } else if let Some(item) = candidates.first() {
                let hash = item.server.hash.clone();
                let result = if let Some(read) = &item.read {
                    self.inner.in_flight.borrow_mut().insert(hash.clone(), Flight::Read(read.clone()));
                    self.call(
                        "forgetSeen",
                        &[(read.generation as f64).into(), hash.clone().into(), (read.expected_revision as f64).into()],
                    )
                    .await
                } else if let Some(bookmark) = &item.bookmark {
                    self.inner.in_flight.borrow_mut().insert(hash.clone(), Flight::Bookmark(bookmark.clone()));
                    self.call(
                        "setBookmark",
                        &[hash.clone().into(), bookmark.desired.into(), (bookmark.expected_revision as f64).into()],
                    )
                    .await
                } else {
                    continue;
                };
                self.finish_flights(std::slice::from_ref(&hash), result.is_ok(), attempt).await?;
                result?;
            }
        }
        Ok(())
    }
    async fn finish_flights(&self, hashes: &[String], accepted: bool, attempt: u64) -> Result<(), String> {
        self.check(attempt)?;
        let fresh = self.membership(hashes).await;
        self.check(attempt)?;
        let rows = match fresh {
            Ok(rows) => rows,
            Err(error) => {
                for hash in hashes {
                    self.inner.in_flight.borrow_mut().remove(hash);
                    let buffered = self.inner.buffered.borrow_mut().remove(hash);
                    if let Some(row) = buffered {
                        self.observe(row);
                    }
                }
                return Err(error);
            }
        };
        for row in rows {
            let hash = row.hash.clone();
            let flight = self.inner.in_flight.borrow_mut().remove(&hash);
            let buffered = self.inner.buffered.borrow_mut().remove(&hash);
            self.library.0.update(|s| {
                let account = s.metadata.effective();
                let c = s.items.entry(hash.clone()).or_insert_with(|| empty(self.fid, &hash, account.generation));
                apply_flight(c, flight, accepted, row, buffered, &account);
                s.revision += 1;
            });
            if let Some(item) = self.library.0.with_untracked(|s| s.items.get(&hash).cloned()) {
                library_disk::keep_item(self.fid, &item).await?;
            }
        }
        Ok(())
    }
    async fn refresh_account(&self) -> Result<(), String> {
        let attempt = self.inner.attempt.get();
        let bridge = self.bridge()?;
        let tag = format!("library-meta-{}", next_id());
        let _lease = Lease { bridge, tag: tag.clone(), hashes: BTreeSet::new() };
        let value = self
            .raw("snapshot", &[tag.into(), "myLibraryState".into(), "[\"SELECT * FROM my_library_state\"]".into()])?;
        let rows: Vec<Account> = parse(crate::notification_sync::bounded(value).await?)?;
        self.check(attempt)?;
        let row = rows.into_iter().find(|r| r.fid == self.fid).ok_or("Private Library metadata is missing")?;
        observe_account(self, row);
        Ok(())
    }
    async fn migrate_legacy(&self, attempt: u64) -> Result<(), String> {
        if self.library.0.with_untracked(|s| s.metadata.legacy_imported) {
            return Ok(());
        }
        let entries = crate::seen::legacy_entries(self.fid);
        for batch in entries.chunks(100) {
            self.check(attempt)?;
            let hashes: Vec<_> = batch.iter().filter_map(|(hash, _)| canonical_hash(hash)).collect();
            if hashes.is_empty() {
                continue;
            }
            let rows = self.membership(&hashes).await?;
            for row in rows {
                let hash = row.hash.clone();
                let at = batch.iter().find(|(h, _)| h == &hash).map_or(0, |(_, at)| *at);
                let sequence = self.sequence();
                self.library.0.update(|s| {
                    let account = s.metadata.effective();
                    let c = s.items.entry(hash.clone()).or_insert_with(|| empty(self.fid, &hash, account.generation));
                    c.observe(row, &account);
                    if !c.pending()
                        && !c.server.seen
                        && c.server.seen_revision == 0
                        && at > account.cleared_at
                        && at <= now()
                    {
                        c.read = Some(PendingRead {
                            generation: account.generation,
                            expected_revision: 0,
                            intent: ReadIntent::Seen { at },
                            sequence,
                        });
                    }
                    s.revision += 1;
                });
                if let Some(item) = self.library.0.with_untracked(|s| s.items.get(&hash).cloned()) {
                    library_disk::keep_item(self.fid, &item).await?;
                }
            }
        }
        let mut meta = self.library.0.with_untracked(|s| s.metadata.clone());
        meta.legacy_imported = true;
        library_disk::keep_account(self.fid, &serde_json::to_string(&meta).map_err(|e| e.to_string())?).await?;
        self.check(attempt)?;
        self.library.0.update(|s| s.metadata.legacy_imported = true);
        Ok(())
    }
}
fn merge_row(current: &mut Item, incoming: Item, account: &Account) {
    let mut cache = CachedItem { server: current.clone(), known: true, ..CachedItem::default() };
    cache.observe(incoming, account);
    *current = cache.server;
}
fn fold_rows(rows: Vec<Item>, fid: u64, generation: u64) -> BTreeMap<String, Item> {
    let account = Account { fid, generation, ..Account::default() };
    let mut folded = BTreeMap::<String, Item>::new();
    for mut row in rows
        .into_iter()
        .filter(|r| r.fid == fid && r.generation == generation && canonical_hash(&r.hash).as_ref() == Some(&r.hash))
    {
        row.cursor.clear();
        if let Some(old) = folded.get_mut(&row.hash) {
            merge_row(old, row, &account);
        } else {
            folded.insert(row.hash.clone(), row);
        }
    }
    folded
}
fn apply_flight(
    cache: &mut CachedItem,
    flight: Option<Flight>,
    accepted: bool,
    row: Item,
    buffered: Option<Item>,
    account: &Account,
) {
    match (accepted, flight) {
        (true, Some(Flight::Read(sent))) => cache.accepted_read(&sent, row, account),
        (true, Some(Flight::Bookmark(sent))) => cache.accepted_bookmark(&sent, row, account),
        _ => cache.observe(row, account),
    }
    // A newer remote operation can arrive after the one-shot response was
    // captured but before its Promise resumes Rust. It remains authoritative.
    if let Some(buffered) = buffered {
        cache.observe(buffered, account);
    }
}
fn clamp_replay(item: &mut CachedItem, now: i64, cleared_at: i64) {
    if let Some(read) = item.read.as_mut()
        && let ReadIntent::Seen { at } = &mut read.intent
    {
        *at = (*at).min(now);
        if *at <= cleared_at {
            let seq = read.sequence;
            item.acknowledged_read(seq);
        }
    }
}
fn stamp(row: &Item, kind: PrivateKind) -> i64 {
    if kind == PrivateKind::Seen { row.seen_at } else { row.bookmark_at }
}
fn valid_cursor(value: &str) -> bool {
    value.is_empty()
        || value
            .split_once(':')
            .is_some_and(|(at, h)| at.parse::<i64>().is_ok_and(|v| v > 0) && canonical_hash(h).as_deref() == Some(h))
}
fn before(row: &Item, after: &str, kind: PrivateKind) -> bool {
    after
        .split_once(':')
        .and_then(|(at, h)| at.parse::<i64>().ok().map(|at| (stamp(row, kind), row.hash.as_str()) < (at, h)))
        .unwrap_or(false)
}

pub(crate) fn account(fid: Option<u64>) {
    let Some(library) = handle() else { return };
    if current().map(|b| b.fid) == fid && library.0.with_untracked(|s| s.fid) == fid {
        return;
    }
    if let Some(b) = CURRENT.with(|c| c.borrow_mut().take()) {
        b.stop();
    }
    crate::pages::library::clear_cached();
    library.0.set(State {
        fid,
        status: if fid.is_some() { SyncStatus::Connecting } else { SyncStatus::SignedOut },
        metadata: Metadata { fid: fid.unwrap_or(0), ..Metadata::default() },
        ..State::default()
    });
    let Some(fid) = fid else { return };
    let b = Broker {
        fid,
        library,
        inner: Rc::new(Inner {
            stopped: Cell::new(false),
            attempt: Cell::new(0),
            bridge: RefCell::new(None),
            base: Cell::new(false),
            disk: Cell::new(false),
            disk_failed: Cell::new(false),
            bootstrapping: Cell::new(false),
            clear_busy: Cell::new(false),
            loading: RefCell::new(BTreeSet::new()),
            groups: RefCell::new(BTreeMap::new()),
            writing: Cell::new(false),
            drain_again: Cell::new(false),
            persisting: Cell::new(false),
            writes: RefCell::new(VecDeque::new()),
            in_flight: RefCell::new(BTreeMap::new()),
            buffered: RefCell::new(BTreeMap::new()),
            busy: RefCell::new(BTreeSet::new()),
            touches: RefCell::new(BTreeMap::new()),
            clock: Cell::new(0),
        }),
    };
    CURRENT.with(|c| *c.borrow_mut() = Some(b.clone()));
    b.load_watched();
    b.bootstrap();
}

pub(crate) fn capability(fid: u64, enabled: Option<bool>) {
    if let Some(b) = current().filter(|b| b.fid == fid) {
        b.library.0.update(|s| {
            s.capability = enabled;
            if enabled == Some(false) {
                s.status = SyncStatus::Failed("Private Library sync is not configured on this server".into());
            }
        });
    }
}
pub(crate) fn bind(fid: u64, attempt: u64, bridge: JsValue) {
    if let Some(b) = current().filter(|b| b.fid == fid) {
        b.inner.groups.borrow_mut().clear();
        b.inner.in_flight.borrow_mut().clear();
        b.inner.buffered.borrow_mut().clear();
        b.inner.attempt.set(attempt);
        b.inner.base.set(false);
        *b.inner.bridge.borrow_mut() = Some(bridge);
        b.library.0.update(|s| {
            s.checked.clear();
            s.status = SyncStatus::Connecting;
        });
    }
}
pub(crate) fn base_applied(fid: u64) -> Result<(), String> {
    let b = current().filter(|b| b.fid == fid).ok_or("Library session changed")?;
    let bridge = b.bridge()?;
    let rows: Vec<Account> = parse(
        method(&bridge, "rows")?.call1(&bridge, &"myLibraryState".into()).map_err(crate::stdb_bridge::js_error)?,
    )?;
    let row = rows.into_iter().find(|r| r.fid == fid).ok_or("Private Library metadata is missing")?;
    b.inner.base.set(true);
    observe_account(&b, row);
    b.ready();
    b.watches();
    b.drain();
    Ok(())
}
fn observe_account(b: &Broker, row: Account) {
    if row.fid != b.fid {
        return;
    }
    let mut changed = false;
    let mut reset = false;
    let mut settled = Vec::new();
    b.library.0.update(|s| {
        if s.metadata.known
            && (row.generation, row.revision) < (s.metadata.server.generation, s.metadata.server.revision)
        {
            return;
        }
        let old = s.metadata.clone();
        reset = row.generation != s.metadata.server.generation;
        let foreign = s
            .metadata
            .pending_clear
            .as_ref()
            .filter(|p| row.generation > p.expected_generation && row.last_clear_id != p.operation)
            .map(|p| p.expected_generation + 1);
        s.metadata.server = row;
        s.metadata.known = true;
        if let Some(p) = s.metadata.pending_clear.clone()
            && s.metadata.server.generation > p.expected_generation
        {
            s.metadata.clear_settled = s.metadata.clear_settled.max(p.sequence);
            s.metadata.pending_clear = None;
        }
        let account = s.metadata.effective();
        for (hash, c) in &mut s.items {
            let previous = c.clone();
            if foreign.is_some_and(|g| c.read.as_ref().is_some_and(|p| p.generation == g))
                && let Some(read) = c.read.take()
            {
                c.acknowledged_read(read.sequence);
            }
            if !b.inner.in_flight.borrow().contains_key(hash) {
                c.reconcile(&account);
            }
            if previous != *c {
                settled.push(hash.clone());
                changed = true;
            }
        }
        changed |= old != s.metadata;
        if changed {
            s.revision += 1;
        }
        if reset {
            s.checked.clear();
        }
    });
    if reset {
        b.inner.groups.borrow_mut().clear();
    }
    if changed {
        b.persist_meta();
    }
    for hash in settled {
        b.persist_item(&hash);
    }
    b.watches();
}
pub(crate) fn event(fid: u64, json: &str) {
    #[derive(Deserialize)]
    struct Event {
        t: String,
        table: String,
        row: Option<serde_json::Value>,
    }
    let Some(b) = current().filter(|b| b.fid == fid) else { return };
    let Ok(e) = serde_json::from_str::<Event>(json) else { return };
    if !matches!(e.t.as_str(), "insert" | "update") {
        return;
    }
    let Some(row) = e.row else { return };
    match e.table.as_str() {
        "my_library_state" => {
            if let Ok(row) = serde_json::from_value(row) {
                observe_account(&b, row);
            }
        }
        "my_library_membership" => {
            if let Ok(row) = serde_json::from_value::<Item>(row)
                && row.fid == fid
            {
                b.observe(row);
            }
        }
        _ => {}
    }
}
pub(crate) fn connection_failed(fid: u64, error: String) {
    if let Some(b) = current().filter(|b| b.fid == fid) {
        b.inner.base.set(false);
        b.inner.groups.borrow_mut().clear();
        b.inner.bridge.borrow_mut().take();
        b.library.0.update(|s| {
            s.checked.clear();
            s.status = if web_sys::window().is_some_and(|w| !w.navigator().on_line()) {
                SyncStatus::Offline
            } else {
                SyncStatus::Failed(error)
            };
            s.revision += 1;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn meta(generation: u64, revision: u64) -> Metadata {
        Metadata {
            fid: 3,
            server: Account { fid: 3, generation, revision, ..Account::default() },
            known: true,
            ..Metadata::default()
        }
    }
    #[test]
    fn disk_metadata_keeps_the_latest_clear_and_never_resurrects_a_settled_operation() {
        let mut current = meta(2, 4);
        current.pending_clear = Some(PendingClear { operation: "a".repeat(32), expected_generation: 2, sequence: 20 });
        let stale = current.clone();
        let mut remote = meta(3, 5);
        remote.clear_settled = 20;
        remote.legacy_imported = true;
        current.merge_disk(&remote);
        current.merge_disk(&stale);
        assert_eq!(current.server.generation, 3);
        assert_eq!(current.server.revision, 5);
        assert!(current.pending_clear.is_none());
        assert!(current.legacy_imported);
        assert_eq!(current.clear_settled, 20);
    }
    #[test]
    fn concurrent_clear_metadata_uses_the_newest_local_sequence_and_preserves_server_revision() {
        let mut first = meta(1, 9);
        first.sequence = 10;
        first.pending_clear = Some(PendingClear { operation: "a".repeat(32), expected_generation: 1, sequence: 10 });
        let mut second = meta(1, 7);
        second.sequence = 11;
        second.pending_clear = Some(PendingClear { operation: "b".repeat(32), expected_generation: 1, sequence: 11 });
        first.merge_disk(&second);
        assert_eq!(first.server.revision, 9);
        assert_eq!(first.pending_clear.as_ref().unwrap().operation, "b".repeat(32));
        assert_eq!(first.effective().generation, 2);
        let foreign = Metadata { fid: 4, ..meta(99, 99) };
        let snapshot = first.clone();
        first.merge_disk(&foreign);
        assert_eq!(first, snapshot);
    }
    #[test]
    fn private_page_cursor_is_a_strict_descending_time_hash_boundary() {
        let hash = format!("0x{:040x}", 9);
        let cursor = format!("100:{hash}");
        assert!(valid_cursor(""));
        assert!(valid_cursor(&cursor));
        for invalid in ["0:x", "-1:x", "100:0xnope", "100:x' OR 1=1", "100:0x000000000000000000000000000000000000000A"]
        {
            assert!(!valid_cursor(invalid));
        }
        let row = Item { hash: hash.clone(), seen_at: 100, bookmark_at: 99, ..Item::default() };
        assert!(!before(&row, &cursor, PrivateKind::Seen));
        assert!(before(&row, &cursor, PrivateKind::Bookmarks));
        assert!(before(&Item { hash: format!("0x{:040x}", 8), ..row }, &cursor, PrivateKind::Seen));
    }
    #[test]
    fn memory_eviction_keeps_active_and_pending_items_and_preserves_recently_touched_rows() {
        let mut items = BTreeMap::new();
        let mut touches = BTreeMap::new();
        for i in 0..2003 {
            let hash = format!("0x{i:040x}");
            items.insert(hash.clone(), empty(3, &hash, 0));
            touches.insert(hash, i);
        }
        let active = format!("0x{:040x}", 0);
        let pending = format!("0x{:040x}", 1);
        items.get_mut(&pending).unwrap().read = Some(PendingRead {
            generation: 0,
            expected_revision: 0,
            intent: ReadIntent::Seen { at: 100 },
            sequence: 1,
        });
        let evicted = evictions(&items, &BTreeSet::from([active.clone()]), &touches);
        assert_eq!(evicted.len(), 3);
        assert!(!evicted.contains(&active));
        assert!(!evicted.contains(&pending));
        assert_eq!(evicted, vec![format!("0x{:040x}", 2), format!("0x{:040x}", 3), format!("0x{:040x}", 4)]);
        assert_eq!(items.get(&pending).unwrap().read.as_ref().unwrap().intent, ReadIntent::Seen { at: 100 });
    }
    #[test]
    fn corrected_clock_replay_clamps_future_observations_but_never_crosses_a_clear() {
        let hash = format!("0x{:040x}", 1);
        let mut item = empty(3, &hash, 2);
        item.read = Some(PendingRead {
            generation: 2,
            expected_revision: 0,
            intent: ReadIntent::Seen { at: 500 },
            sequence: 17,
        });
        clamp_replay(&mut item, 200, 100);
        assert_eq!(item.read.as_ref().unwrap().intent, ReadIntent::Seen { at: 200 });
        clamp_replay(&mut item, 100, 100);
        assert!(item.read.is_none());
        assert_eq!(item.read_settled, 17);
    }
    #[test]
    fn automatic_seen_and_explicit_bookmark_have_independent_pending_domains() {
        let mut item = empty(3, &format!("0x{:040x}", 1), 0);
        item.read = Some(PendingRead {
            generation: 0,
            expected_revision: 0,
            intent: ReadIntent::Seen { at: 100 },
            sequence: 1,
        });
        assert!(Action::Unread.pending(&item));
        assert!(!Action::Toggle.pending(&item));
        item.bookmark = Some(PendingBookmark { desired: true, expected_revision: 0, sequence: 2 });
        assert!(Action::Toggle.pending(&item));
        item.read = None;
        assert!(!Action::Seen(101).pending(&item));
    }
    #[test]
    fn sdk_union_fold_is_order_independent_for_read_and_bookmark_revisions() {
        let hash = format!("0x{:040x}", 1);
        let base = Item { fid: 3, hash: hash.clone(), ..Item::default() };
        let read = Item { seen: true, seen_at: 100, seen_revision: 4, bookmark_revision: 1, ..base.clone() };
        let bookmark =
            Item { bookmarked: true, bookmark_at: 101, bookmark_revision: 5, seen_revision: 2, ..base.clone() };
        let forward = fold_rows(vec![read.clone(), bookmark.clone()], 3, 0);
        let reverse = fold_rows(vec![bookmark.clone(), read.clone(), base.clone()], 3, 0);
        assert_eq!(forward, reverse);
        let row = &forward[&hash];
        assert!(row.seen && row.bookmarked);
        assert_eq!(row.seen_revision, 4);
        assert_eq!(row.bookmark_revision, 5);
        assert!(fold_rows(vec![Item { fid: 4, ..base.clone() }, Item { generation: 1, ..base }], 3, 0).is_empty());
    }
    #[test]
    fn buffered_remote_remove_after_own_ack_cannot_be_lost_or_restore_old_pending() {
        let hash = format!("0x{:040x}", 1);
        let base = Item { fid: 3, hash, ..Item::default() };
        let account = Account { fid: 3, ..Account::default() };
        let sent = PendingBookmark { desired: true, expected_revision: 0, sequence: 1 };
        let mut cache =
            CachedItem { server: base.clone(), known: true, bookmark: Some(sent.clone()), ..CachedItem::default() };
        let own = Item { bookmarked: true, bookmark_revision: 1, bookmark_at: 100, ..base.clone() };
        let remote = Item { bookmark_revision: 2, ..base.clone() };
        apply_flight(&mut cache, Some(Flight::Bookmark(sent)), true, own, Some(remote), &account);
        assert_eq!(cache.bookmarked(), Some(false));
        assert_eq!(cache.server.bookmark_revision, 2);
        assert!(cache.bookmark.is_none());
        assert_eq!(cache.bookmark_settled, 1);
    }
}

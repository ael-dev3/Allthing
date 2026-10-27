// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable private Library cache and outbox. Point reads never enumerate the
//! history; an outbox page scans only one FID's pending-key range. A write is
//! acknowledged only by transaction completion, after both copies commit.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use allthing_core::library::{CachedItem, canonical_hash};
use js_sys::Array;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Event, IdbDatabase, IdbKeyRange, IdbOpenDbRequest, IdbRequest, IdbTransaction, IdbTransactionMode};

const DATABASE: &str = "allthing-library-v1";
const STORE: &str = "records";
const MAX_RECORD: usize = 65_536;
const MAX_PENDING: u32 = 100;

fn error(value: JsValue) -> String {
    crate::stdb_bridge::js_error(value)
}

struct Completion<T> {
    settled: bool,
    result: Option<Result<T, String>>,
    waker: Option<Waker>,
}

impl<T> Default for Completion<T> {
    fn default() -> Self {
        Self { settled: false, result: None, waker: None }
    }
}

impl<T> Completion<T> {
    fn finish(state: &RefCell<Self>, result: Result<T, String>) {
        let waker = {
            let mut state = state.borrow_mut();
            if state.settled {
                return;
            }
            state.settled = true;
            state.result = Some(result);
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll(state: &RefCell<Self>, cx: &Context<'_>) -> Poll<Result<T, String>> {
        let mut state = state.borrow_mut();
        if let Some(result) = state.result.take() {
            Poll::Ready(result)
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// A connection belongs to this operation, including a version-change closer.
/// Closing it does not cancel committed data or another operation's connection.
struct Database {
    db: IdbDatabase,
    changed: Closure<dyn FnMut()>,
}

impl Database {
    fn new(db: IdbDatabase) -> Result<Self, String> {
        let close = db.clone();
        let changed = Closure::new(move || close.close());
        if let Err(failed) = db.add_event_listener_with_callback("versionchange", changed.as_ref().unchecked_ref()) {
            db.close();
            return Err(error(failed));
        }
        Ok(Self { db, changed })
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = self.db.remove_event_listener_with_callback("versionchange", self.changed.as_ref().unchecked_ref());
        self.db.close();
    }
}

/// IndexedDB cannot cancel a pending open request. Normal listeners are owned;
/// cancellation replaces them with GC-owned one-shot upgrade-abort/success-close
/// listeners with no captured request/self cycle, so a late open leaks no DB.
struct Opening {
    request: IdbOpenDbRequest,
    state: Rc<RefCell<Completion<IdbDatabase>>>,
    finished: Rc<std::cell::Cell<bool>>,
    success: Closure<dyn FnMut()>,
    failed: Closure<dyn FnMut()>,
    blocked: Closure<dyn FnMut()>,
    upgrade: Closure<dyn FnMut()>,
    timeout: Closure<dyn FnMut()>,
    timer: Option<JsValue>,
}

impl Opening {
    fn new() -> Result<Self, String> {
        let factory = web_sys::window()
            .ok_or("No browser window")?
            .indexed_db()
            .map_err(error)?
            .ok_or("Library storage is unavailable")?;
        let request = factory.open_with_u32(DATABASE, 1).map_err(error)?;
        let state = Rc::new(RefCell::new(Completion::default()));
        let finished = Rc::new(std::cell::Cell::new(false));
        let success_state = Rc::downgrade(&state);
        let success_request = request.clone();
        let success_finished = finished.clone();
        let failed_state = Rc::downgrade(&state);
        let failed_finished = finished.clone();
        let failed_request = request.clone();
        let blocked_state = Rc::downgrade(&state);
        let upgrade_state = Rc::downgrade(&state);
        let upgrade_request = request.clone();
        let timeout_state = Rc::downgrade(&state);
        let mut opening = Self {
            request,
            state,
            finished,
            success: Closure::new(move || {
                success_finished.set(true);
                let result = success_request
                    .result()
                    .map_err(error)
                    .and_then(|value| value.dyn_into::<IdbDatabase>().map_err(error));
                if let Some(state) = success_state.upgrade() {
                    Completion::finish(&state, result);
                } else if let Ok(db) = result {
                    db.close();
                }
            }),
            failed: Closure::new(move || {
                failed_finished.set(true);
                if let Some(state) = failed_state.upgrade() {
                    let message = failed_request
                        .error()
                        .ok()
                        .flatten()
                        .map(|error| error.message())
                        .unwrap_or_else(|| "Couldn't open Library storage".into());
                    Completion::finish(&state, Err(message));
                }
            }),
            blocked: Closure::new(move || {
                if let Some(state) = blocked_state.upgrade() {
                    Completion::finish(&state, Err("Library storage is blocked by another tab".into()));
                }
            }),
            upgrade: Closure::new(move || {
                let result = upgrade_request
                    .result()
                    .map_err(error)
                    .and_then(|value| value.dyn_into::<IdbDatabase>().map_err(error))
                    .and_then(|db| db.create_object_store(STORE).map(|_| ()).map_err(error));
                if let Err(error) = result {
                    if let Some(tx) = upgrade_request.transaction() {
                        let _ = tx.abort();
                    }
                    if let Some(state) = upgrade_state.upgrade() {
                        Completion::finish(&state, Err(error));
                    }
                }
            }),
            timeout: Closure::new(move || {
                if let Some(state) = timeout_state.upgrade() {
                    Completion::finish(&state, Err("Library storage took too long to open".into()));
                }
            }),
            timer: None,
        };
        for (name, callback) in opening.listeners() {
            opening.request.add_event_listener_with_callback(name, callback).map_err(error)?;
        }
        opening.timer = Some(crate::stdb_bridge::set_timeout(opening.timeout.as_ref(), 30_000)?);
        Ok(opening)
    }

    fn listeners(&self) -> [(&'static str, &js_sys::Function); 4] {
        [
            ("success", self.success.as_ref().unchecked_ref()),
            ("error", self.failed.as_ref().unchecked_ref()),
            ("blocked", self.blocked.as_ref().unchecked_ref()),
            ("upgradeneeded", self.upgrade.as_ref().unchecked_ref()),
        ]
    }
}

impl Future for Opening {
    type Output = Result<IdbDatabase, String>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Completion::poll(&self.state, cx)
    }
}

impl Drop for Opening {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            crate::stdb_bridge::clear_timeout(&timer);
        }
        for (name, callback) in self.listeners() {
            let _ = self.request.remove_event_listener_with_callback(name, callback);
        }
        if let Some(Ok(db)) = self.state.borrow_mut().result.take() {
            db.close();
        }
        if self.finished.get() {
            return;
        }
        if let Some(tx) = self.request.transaction() {
            let _ = tx.abort();
        }
        let once = web_sys::AddEventListenerOptions::new();
        once.set_once(true);
        let close = Closure::<dyn FnMut(Event)>::new(|event: Event| {
            if let Some(request) = event.target().and_then(|target| target.dyn_into::<IdbRequest>().ok())
                && let Ok(db) = request.result().and_then(|value| value.dyn_into::<IdbDatabase>())
            {
                db.close();
            }
        })
        .into_js_value();
        let abort = Closure::<dyn FnMut(Event)>::new(|event: Event| {
            if let Some(request) = event.target().and_then(|target| target.dyn_into::<IdbRequest>().ok())
                && let Some(tx) = request.transaction()
            {
                let _ = tx.abort();
            }
        })
        .into_js_value();
        let _ = self.request.add_event_listener_with_callback_and_add_event_listener_options(
            "success",
            close.unchecked_ref(),
            &once,
        );
        let _ = self.request.add_event_listener_with_callback_and_add_event_listener_options(
            "upgradeneeded",
            abort.unchecked_ref(),
            &once,
        );
    }
}

struct Transaction {
    database: Database,
    tx: IdbTransaction,
    state: Rc<RefCell<Completion<Vec<JsValue>>>>,
    requests: Rc<RefCell<Vec<IdbRequest>>>,
    callbacks: Vec<(IdbRequest, Closure<dyn FnMut()>)>,
    complete: Closure<dyn FnMut()>,
    failed: Closure<dyn FnMut()>,
}

impl Transaction {
    fn new(database: Database, mode: IdbTransactionMode) -> Result<Self, String> {
        let tx = database.db.transaction_with_str_and_mode(STORE, mode).map_err(error)?;
        let state = Rc::new(RefCell::new(Completion::default()));
        let requests = Rc::new(RefCell::new(Vec::<IdbRequest>::new()));
        let completed_state = Rc::downgrade(&state);
        let completed_requests = requests.clone();
        let failed_state = Rc::downgrade(&state);
        let failed_tx = tx.clone();
        let transaction = Self {
            database,
            tx,
            state,
            requests,
            callbacks: Vec::new(),
            complete: Closure::new(move || {
                if let Some(state) = completed_state.upgrade() {
                    Completion::finish(
                        &state,
                        completed_requests.borrow().iter().map(|request| request.result().map_err(error)).collect(),
                    );
                }
            }),
            failed: Closure::new(move || {
                if let Some(state) = failed_state.upgrade() {
                    let message = failed_tx
                        .error()
                        .map(|error| error.message())
                        .unwrap_or_else(|| "Library storage transaction aborted".into());
                    Completion::finish(&state, Err(message));
                }
            }),
        };
        for (name, callback) in transaction.listeners() {
            transaction.tx.add_event_listener_with_callback(name, callback).map_err(error)?;
        }
        Ok(transaction)
    }
    fn listeners(&self) -> [(&'static str, &js_sys::Function); 3] {
        [
            ("complete", self.complete.as_ref().unchecked_ref()),
            ("error", self.failed.as_ref().unchecked_ref()),
            ("abort", self.failed.as_ref().unchecked_ref()),
        ]
    }
    fn store(&self) -> Result<web_sys::IdbObjectStore, String> {
        self.tx.object_store(STORE).map_err(error)
    }
    fn read(&self, request: IdbRequest) {
        self.requests.borrow_mut().push(request);
    }
    fn on_success(&mut self, request: IdbRequest, callback: Closure<dyn FnMut()>) -> Result<(), String> {
        request.add_event_listener_with_callback("success", callback.as_ref().unchecked_ref()).map_err(error)?;
        self.callbacks.push((request, callback));
        Ok(())
    }
}

impl Future for Transaction {
    type Output = Result<Vec<JsValue>, String>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Completion::poll(&self.state, cx)
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        for (name, callback) in self.listeners() {
            let _ = self.tx.remove_event_listener_with_callback(name, callback);
        }
        for (request, callback) in &self.callbacks {
            let _ = request.remove_event_listener_with_callback("success", callback.as_ref().unchecked_ref());
        }
        if !self.state.borrow().settled {
            let _ = self.tx.abort();
        }
        // Keep the connection owned through commit/error/cancellation.
        self.database.db.close();
    }
}

async fn transaction(mode: IdbTransactionMode) -> Result<Transaction, String> {
    Transaction::new(Database::new(Opening::new()?.await?)?, mode)
}

fn prefix(kind: &str, fid: u64) -> Result<String, String> {
    if fid == 0 {
        return Err("Library storage requires a signed-in account".into());
    }
    Ok(format!("{kind}:{fid:020}:"))
}

fn item_key(kind: &str, fid: u64, hash: &str) -> Result<String, String> {
    if canonical_hash(hash).as_deref() != Some(hash) {
        return Err("Invalid Library cast hash".into());
    }
    Ok(format!("{}{hash}", prefix(kind, fid)?))
}

fn encoded_item(fid: u64, item: &CachedItem) -> Result<String, String> {
    item_key("cache", fid, &item.server.hash)?;
    if item.server.fid != fid {
        return Err("Library cache belongs to another account".into());
    }
    let json = serde_json::to_string(item).map_err(|_| "Couldn't encode Library storage".to_owned())?;
    if json.len() > MAX_RECORD {
        return Err("Library record is too large".into());
    }
    Ok(json)
}

fn decoded_item(fid: u64, value: JsValue) -> Result<CachedItem, String> {
    let json = value.as_string().ok_or("Unreadable Library cache record")?;
    if json.len() > MAX_RECORD {
        return Err("Library record is too large".into());
    }
    let item: CachedItem = serde_json::from_str(&json).map_err(|_| "Unreadable Library cache record")?;
    encoded_item(fid, &item)?;
    Ok(item)
}

/// Read only the requested points; a cold feed never loads the entire history.
pub async fn items(fid: u64, hashes: &[String]) -> Result<Vec<CachedItem>, String> {
    prefix("cache", fid)?;
    let mut unique = std::collections::HashSet::new();
    let keys: Vec<_> = hashes
        .iter()
        .filter(|hash| unique.insert(hash.as_str()))
        .map(|hash| item_key("cache", fid, hash).map(|key| (hash, key)))
        .collect::<Result<_, _>>()?;
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let tx = transaction(IdbTransactionMode::Readonly).await?;
    let store = tx.store()?;
    for (_, key) in &keys {
        tx.read(store.get(&JsValue::from_str(key)).map_err(error)?);
    }
    let found = tx.await?;
    let mut items = Vec::new();
    for ((hash, _), value) in keys.into_iter().zip(found) {
        if value.is_undefined() {
            continue;
        }
        let item = decoded_item(fid, value)?;
        if item.server.hash != *hash {
            return Err("Library cache hash does not match its key".into());
        }
        items.push(item);
    }
    Ok(items)
}

/// Cache and outbox updates share one commit; an aborted/quota-limited write
/// cannot leave one new while the other is old. No pending item is evicted.
pub async fn keep_item(fid: u64, item: &CachedItem) -> Result<(), String> {
    encoded_item(fid, item)?;
    let incoming = item.clone();
    let (cache, pending) = (item_key("cache", fid, &item.server.hash)?, item_key("pending", fid, &item.server.hash)?);
    let mut tx = transaction(IdbTransactionMode::Readwrite).await?;
    let store = tx.store()?;
    let request = store.get(&JsValue::from_str(&cache)).map_err(error)?;
    let read = request.clone();
    let state = Rc::downgrade(&tx.state);
    let abort = tx.tx.clone();
    let callback = Closure::<dyn FnMut()>::new(move || {
        // This callback runs while IDB's read/write lock is still active.
        // Queue both writes synchronously; awaiting between them could commit
        // the transaction before the second tab's merge is durable.
        let merged = (|| -> Result<(), String> {
            let previous = read.result().map_err(error)?;
            let item = if previous.is_undefined() {
                incoming.clone()
            } else {
                let mut previous = decoded_item(fid, previous)?;
                if previous.server.hash != incoming.server.hash {
                    return Err("Library cache hash does not match its key".into());
                }
                previous.merge_disk(&incoming);
                previous
            };
            // Keep validation and serialization after the merge as well.
            let json = encoded_item(fid, &item)?;
            store.put_with_key(&JsValue::from_str(&json), &JsValue::from_str(&cache)).map_err(error)?;
            if item.pending() {
                store.put_with_key(&JsValue::from_str(&json), &JsValue::from_str(&pending)).map_err(error)?;
            } else {
                store.delete(&JsValue::from_str(&pending)).map_err(error)?;
            }
            Ok(())
        })();
        if let Err(error) = merged {
            if let Some(state) = state.upgrade() {
                Completion::finish(&state, Err(error));
            }
            let _ = abort.abort();
        }
    });
    tx.on_success(request, callback)?;
    tx.await.map(|_| ())
}

/// Scan just this account's outbox, at most100 records; the cache can be much
/// larger. A caller drains later pages by acknowledging/removing pending intent.
pub async fn pending(fid: u64, limit: u32) -> Result<Vec<CachedItem>, String> {
    let lower = prefix("pending", fid)?;
    let limit = limit.min(MAX_PENDING);
    if limit == 0 {
        return Ok(Vec::new());
    }
    let range = IdbKeyRange::bound(&JsValue::from_str(&lower), &JsValue::from_str(&format!("{lower}\u{ffff}")))
        .map_err(error)?;
    let tx = transaction(IdbTransactionMode::Readonly).await?;
    tx.read(tx.store()?.get_all_with_key_and_limit(range.as_ref(), limit).map_err(error)?);
    let rows = tx.await?;
    let values = Array::from(rows.first().ok_or("Library outbox did not answer")?);
    let mut found = Vec::new();
    for value in values.iter() {
        let item = decoded_item(fid, value)?;
        if !item.pending() {
            return Err("Library outbox contains a settled record".into());
        }
        found.push(item);
    }
    Ok(found)
}

fn account_json(fid: u64, json: &str) -> Result<(), String> {
    prefix("account", fid)?;
    if json.len() > MAX_RECORD {
        return Err("Library account metadata is too large".into());
    }
    let value: serde_json::Value = serde_json::from_str(json).map_err(|_| "Unreadable Library account metadata")?;
    if !value.is_object() || value.get("fid").and_then(serde_json::Value::as_u64) != Some(fid) {
        return Err("Library account metadata belongs to another account".into());
    }
    Ok(())
}

pub async fn account(fid: u64) -> Result<Option<String>, String> {
    let key = prefix("account", fid)?;
    let tx = transaction(IdbTransactionMode::Readonly).await?;
    tx.read(tx.store()?.get(&JsValue::from_str(&key)).map_err(error)?);
    let rows = tx.await?;
    let value = rows.first().ok_or("Library account metadata did not answer")?;
    if value.is_undefined() {
        return Ok(None);
    }
    let json = value.as_string().ok_or("Unreadable Library account metadata")?;
    account_json(fid, &json)?;
    Ok(Some(json))
}

pub async fn keep_account(fid: u64, value: &str) -> Result<(), String> {
    account_json(fid, value)?;
    let incoming: crate::library::Metadata =
        serde_json::from_str(value).map_err(|_| "Unreadable Library account metadata")?;
    let key = prefix("account", fid)?;
    let mut tx = transaction(IdbTransactionMode::Readwrite).await?;
    let store = tx.store()?;
    let request = store.get(&JsValue::from_str(&key)).map_err(error)?;
    let read = request.clone();
    let state = Rc::downgrade(&tx.state);
    let abort = tx.tx.clone();
    let callback = Closure::<dyn FnMut()>::new(move || {
        let result = (|| -> Result<(), String> {
            let previous = read.result().map_err(error)?;
            let mut merged = if previous.is_undefined() {
                incoming.clone()
            } else {
                let json = previous.as_string().ok_or("Unreadable Library account metadata")?;
                account_json(fid, &json)?;
                let mut previous: crate::library::Metadata =
                    serde_json::from_str(&json).map_err(|_| "Unreadable Library account metadata")?;
                previous.merge_disk(&incoming);
                previous
            };
            merged.fid = fid;
            let json = serde_json::to_string(&merged).map_err(|e| e.to_string())?;
            account_json(fid, &json)?;
            store.put_with_key(&JsValue::from_str(&json), &JsValue::from_str(&key)).map_err(error)?;
            Ok(())
        })();
        if let Err(error) = result {
            if let Some(state) = state.upgrade() {
                Completion::finish(&state, Err(error));
            }
            let _ = abort.abort();
        }
    });
    tx.on_success(request, callback)?;
    tx.await.map(|_| ())
}

fn moderation_key(fid: u64, target: u64) -> Result<String, String> {
    if target == 0 || target == fid {
        return Err("Invalid account filter target".into());
    }
    Ok(format!("{}{target:020}", prefix("moderation", fid)?))
}

fn decode_moderation(fid: u64, raw: JsValue) -> Result<crate::moderation::Stored, String> {
    let json = raw.as_string().ok_or("Unreadable account preference")?;
    if json.len() > MAX_RECORD {
        return Err("Account preference record is too large".into());
    }
    let item: crate::moderation::Stored = serde_json::from_str(&json).map_err(|_| "Unreadable account preference")?;
    moderation_key(fid, item.server.target_fid)?;
    if item.server.fid != fid {
        return Err("Account preference belongs to another account".into());
    }
    Ok(item)
}

/// Private account filters occupy their own typed key range in the existing
/// database. No schema upgrade, history scan, eviction or expiry is involved.
pub(crate) async fn moderation_items(fid: u64) -> Result<Vec<crate::moderation::Stored>, String> {
    let lower = prefix("moderation", fid)?;
    let range = IdbKeyRange::bound(&JsValue::from_str(&lower), &JsValue::from_str(&format!("{lower}\u{ffff}")))
        .map_err(error)?;
    let tx = transaction(IdbTransactionMode::Readonly).await?;
    tx.read(tx.store()?.get_all_with_key(range.as_ref()).map_err(error)?);
    let rows = tx.await?;
    Array::from(rows.first().ok_or("Account preferences did not answer")?)
        .iter()
        .map(|row| decode_moderation(fid, row))
        .collect()
}

/// Allocate a new user intent under IDB's read/write lock. Concurrent tabs
/// cannot overwrite independent fields or reuse a per-record sequence. Cache
/// updates and acknowledgements merge under the same lock and commit boundary.
pub(crate) async fn keep_moderation(
    fid: u64,
    incoming: &crate::moderation::Stored,
    action: Option<(u8, bool, u64)>,
) -> Result<crate::moderation::Stored, String> {
    if incoming.server.fid != fid || action.is_some_and(|(kind, _, _)| kind > 1) {
        return Err("Invalid account preference".into());
    }
    let key = moderation_key(fid, incoming.server.target_fid)?;
    let incoming = incoming.clone();
    let mut tx = transaction(IdbTransactionMode::Readwrite).await?;
    let store = tx.store()?;
    let request = store.get(&JsValue::from_str(&key)).map_err(error)?;
    let read = request.clone();
    let state = Rc::downgrade(&tx.state);
    let abort = tx.tx.clone();
    let output = Rc::new(RefCell::new(None));
    let result = output.clone();
    let callback = Closure::<dyn FnMut()>::new(move || {
        let merged = (|| -> Result<(), String> {
            let previous = read.result().map_err(error)?;
            let mut item = if previous.is_undefined() {
                incoming.clone()
            } else {
                let mut item = decode_moderation(fid, previous)?;
                item.merge_disk(&incoming);
                item
            };
            if let Some((kind, desired, expected_revision)) = action {
                item.set_intent_at(kind, desired, expected_revision)?;
            }
            let json = serde_json::to_string(&item).map_err(|e| e.to_string())?;
            if json.len() > MAX_RECORD {
                return Err("Account preference record is too large".into());
            }
            store.put_with_key(&JsValue::from_str(&json), &JsValue::from_str(&key)).map_err(error)?;
            *result.borrow_mut() = Some(item);
            Ok(())
        })();
        if let Err(error) = merged {
            if let Some(state) = state.upgrade() {
                Completion::finish(&state, Err(error));
            }
            let _ = abort.abort();
        }
    });
    tx.on_success(request, callback)?;
    tx.await?;
    let saved = output.borrow_mut().take();
    saved.ok_or_else(|| "Account preference was not committed".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::task::Wake;

    struct Woken(AtomicUsize);
    impl Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn aborted_commit_cannot_be_replaced_by_a_late_success_callback() {
        let woke = Arc::new(Woken(AtomicUsize::new(0)));
        let state =
            RefCell::new(Completion::<usize> { waker: Some(Waker::from(woke.clone())), ..Completion::default() });
        Completion::finish(&state, Err("QuotaExceededError".into()));
        Completion::finish(&state, Ok(7));
        assert_eq!(state.borrow_mut().result.take(), Some(Err("QuotaExceededError".into())));
        assert!(state.borrow().settled);
        assert_eq!(woke.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn completion_before_the_first_poll_is_retained() {
        let state = RefCell::new(Completion::default());
        Completion::finish(&state, Ok(9usize));
        let woke = Arc::new(Woken(AtomicUsize::new(0)));
        let waker = Waker::from(woke.clone());
        assert_eq!(Completion::poll(&state, &Context::from_waker(&waker)), Poll::Ready(Ok(9)));
        assert!(state.borrow().settled);
        assert_eq!(woke.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn outbox_prefixes_are_account_exact_and_records_are_typed() {
        let hash = format!("0x{:040x}", 1);
        let key = item_key("pending", 7, &hash).unwrap();
        assert!(key.starts_with(&prefix("pending", 7).unwrap()));
        for other in [1, 70, 71, u64::MAX] {
            assert!(!key.starts_with(&prefix("pending", other).unwrap()));
        }
        assert!(item_key("cache", 0, &hash).is_err());
        assert!(item_key("cache", 7, "malformed").is_err());
        let mut item = CachedItem::default();
        item.server.fid = 7;
        item.server.hash = hash;
        assert!(encoded_item(7, &item).is_ok());
        assert!(encoded_item(8, &item).is_err());
        assert!(account_json(7, r#"{"fid":7,"server":{"generation":1},"operation_sequence":20}"#).is_ok());
        for json in [r#"{"fid":8}"#, r#"{"fid":"7"}"#, "[]", "{"] {
            assert!(account_json(7, json).is_err());
        }
        assert!(account_json(7, &format!(r#"{{"fid":7,"padding":"{}"}}"#, "x".repeat(MAX_RECORD))).is_err());
    }
}

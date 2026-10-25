// SPDX-License-Identifier: AGPL-3.0-or-later
//! A private, account-wide notification watermark. This connection never
//! subscribes to chat, publishes message keys or changes public presence.

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::time::Duration;

use allthing_core::notification_state::ReadState;
use allthing_core::stdb::{AttestRequest, Attestation, Called, STDB_DATABASE};
use js_sys::{Array, Promise, Reflect};
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use wasm_bindgen::{JsValue, closure::Closure};

use crate::api;
use crate::components::notifications::Notifications;
use crate::keys::{self, NotificationIdentity};
use crate::stdb_bridge::{await_promise, clear_timeout, js_error, load_bridge, method, parse, set_timeout, uri};

const BRIDGE: Option<&str> = option_env!("ALLTHING_STDB_BRIDGE");
const QUERY: &str = "[\"SELECT * FROM my_notification_state\"]";
type Emit = Closure<dyn FnMut(String)>;

thread_local! {
    static CURRENT: RefCell<Option<Sync>> = const { RefCell::new(None) };
}

#[derive(Clone)]
struct Sync {
    fid: u64,
    notifications: Notifications,
    inner: Arc<SendWrapper<Inner>>,
}

struct Inner {
    not_before: f64,
    stopped: Cell<bool>,
    connecting: Cell<bool>,
    disconnected: Cell<bool>,
    ready: Cell<bool>,
    writing: Cell<bool>,
    attempt: Cell<u64>,
    failures: Cell<u32>,
    state: RefCell<ReadState>,
    credentials: RefCell<Option<NotificationIdentity>>,
    bridge: RefCell<Option<JsValue>>,
    emit: RefCell<Option<Emit>>,
    timer: RefCell<Option<TimeoutHandle>>,
}

fn now() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

fn visible_online() -> bool {
    web_sys::window().is_some_and(|w| w.navigator().on_line()) && !crate::util::document().hidden()
}

/// Called by the app's account effect, including sign-out. A stopped
/// instance fences every asynchronous completion, even after A → B → A.
pub fn account(fid: Option<u64>, notifications: Notifications) {
    if CURRENT.with(|s| s.borrow().as_ref().map(|s| s.fid)) == fid {
        return;
    }
    if let Some(old) = CURRENT.with(|s| s.borrow_mut().take()) {
        old.stop();
    }
    let Some(fid) = fid else { return };
    let sync = Sync {
        fid,
        notifications,
        inner: Arc::new(SendWrapper::new(Inner {
            not_before: js_sys::Date::now() + 2000.0,
            stopped: Cell::new(false),
            connecting: Cell::new(false),
            disconnected: Cell::new(false),
            ready: Cell::new(false),
            writing: Cell::new(false),
            attempt: Cell::new(0),
            failures: Cell::new(0),
            state: RefCell::new(ReadState::new(notifications.seen_by(fid), now())),
            credentials: RefCell::new(None),
            bridge: RefCell::new(None),
            emit: RefCell::new(None),
            timer: RefCell::new(None),
        })),
    };
    CURRENT.with(|s| *s.borrow_mut() = Some(sync.clone()));
    sync.schedule(2000);
}

/// A local read or another tab's cache update. The acknowledged server
/// watermark is kept separate, so a failed write never loses its retry.
pub fn seen(fid: u64, at: i64) {
    if let Some(sync) = CURRENT.with(|s| s.borrow().clone()).filter(|s| s.fid == fid) {
        sync.inner.state.borrow_mut().observe_local(at, now());
        if sync.inner.ready.get() && !sync.inner.writing.get() {
            sync.schedule(200);
        }
    }
}

pub fn wake() {
    if let Some(sync) = CURRENT.with(|s| s.borrow().clone()) {
        if sync.inner.ready.get() {
            sync.flush();
        } else if !sync.inner.connecting.get() {
            sync.schedule(0);
        }
    }
}

impl Sync {
    fn active(&self) -> bool {
        !self.inner.stopped.get() && self.notifications.are_for(self.fid)
    }

    fn clear_timer(&self) {
        if let Some(timer) = self.inner.timer.borrow_mut().take() {
            timer.clear();
        }
    }

    fn schedule(&self, millis: u64) {
        self.clear_timer();
        if !self.active() {
            return;
        }
        let sync = self.clone();
        let timer = set_timeout_with_handle(
            move || {
                sync.inner.timer.borrow_mut().take();
                if !sync.active() || !visible_online() {
                    return;
                }
                if sync.inner.ready.get() {
                    sync.flush();
                } else if !api::quiet() {
                    sync.schedule(250);
                } else {
                    crate::components::cast::idle(move || sync.connect());
                }
            },
            Duration::from_millis(millis.max((self.inner.not_before - js_sys::Date::now()).max(0.0).ceil() as u64)),
        );
        *self.inner.timer.borrow_mut() = timer.ok();
    }

    fn detach(&self) {
        if let Some(bridge) = self.inner.bridge.borrow_mut().take()
            && let Ok(disconnect) = method(&bridge, "disconnect")
        {
            let _ = disconnect.call0(&bridge);
        }
        // Bridge.disconnect releases this callback before closing its socket.
        self.inner.emit.borrow_mut().take();
    }

    fn stop(&self) {
        self.inner.stopped.set(true);
        self.inner.ready.set(false);
        self.clear_timer();
        self.detach();
    }

    fn retry(&self) {
        self.inner.ready.set(false);
        if !self.active() || self.inner.connecting.get() || self.inner.writing.get() {
            return;
        }
        self.detach();
        let failures = self.inner.failures.get().saturating_add(1);
        self.inner.failures.set(failures);
        let delay = [5000, 15000, 60000, 180000, 300000][failures.saturating_sub(1).min(4) as usize];
        self.schedule(delay);
    }

    fn connect(&self) {
        if !self.active() || !visible_online() || self.inner.connecting.replace(true) {
            return;
        }
        self.inner.disconnected.set(false);
        self.inner.attempt.set(self.inner.attempt.get().wrapping_add(1));
        let sync = self.clone();
        leptos::task::spawn_local(async move {
            let result = sync.start().await;
            sync.inner.connecting.set(false);
            if !sync.active() {
                sync.detach();
                return;
            }
            if result.is_err() {
                sync.retry();
                return;
            }
            sync.inner.failures.set(0);
            sync.inner.ready.set(true);
            sync.flush();
        });
    }

    fn check(&self) -> Result<(), String> {
        if self.active() && !self.inner.disconnected.get() {
            Ok(())
        } else {
            Err("Session changed or disconnected".into())
        }
    }

    async fn start(&self) -> Result<(), String> {
        // The existing status response advertises the pinned attestation
        // service. Unconfigured servers/fixtures never load the SDK or dial.
        #[derive(serde::Deserialize)]
        struct Status {
            spacetime: Capability,
        }
        #[derive(serde::Deserialize)]
        struct Capability {
            attest: bool,
        }
        let status = api::get::<Status>("/api/v1/status").await.map_err(|e| e.message())?;
        self.check()?;
        if !status.data.spacetime.attest {
            return Err("Notification sync is not configured".into());
        }
        if self.inner.credentials.borrow().is_none() {
            let stored = keys::notification_identity(self.fid).await.ok().flatten();
            self.check()?;
            *self.inner.credentials.borrow_mut() = stored;
        }
        let global = load_bridge(BRIDGE.ok_or("Notification sync is not part of this build")?).await?;
        self.check()?;
        let weak = Arc::downgrade(&self.inner);
        let notifications = self.notifications;
        let fid = self.fid;
        let attempt = self.inner.attempt.get();
        let emit = Closure::<dyn FnMut(String)>::new(move |json: String| {
            let Some(inner) = weak.upgrade() else { return };
            let sync = Sync { fid, notifications, inner };
            if sync.active() && sync.inner.attempt.get() == attempt {
                sync.event(&json);
            }
        });
        let token = self.inner.credentials.borrow().as_ref().map(|s| s.token.clone()).unwrap_or_default();
        let bridge = method(&global, "connect")?
            .apply(&global, &Array::of4(&uri().into(), &STDB_DATABASE.into(), &token.into(), emit.as_ref()))
            .map_err(js_error)?;
        *self.inner.bridge.borrow_mut() = Some(bridge.clone());
        *self.inner.emit.borrow_mut() = Some(emit);
        #[derive(serde::Deserialize)]
        struct Connected {
            identity: String,
            token: String,
        }
        let connected: Connected =
            parse(bounded(Reflect::get(&bridge, &"connected".into()).map_err(js_error)?).await?)?;
        self.check()?;
        if !allthing_core::stdb::is_identity(&connected.identity) {
            return Err("Invalid notification identity".into());
        }
        let stored =
            NotificationIdentity { fid: self.fid, identity: connected.identity.clone(), token: connected.token };
        *self.inner.credentials.borrow_mut() = Some(stored.clone());
        let _ = keys::keep_notification_identity(&stored).await; // memory still works when storage is denied
        self.check()?;
        let att =
            api::post::<_, Attestation>("/api/v1/auth/attest", &AttestRequest { identity: connected.identity.clone() })
                .await
                .map_err(|e| e.message())?;
        self.check()?;
        if att.fid != self.fid || att.identity != connected.identity {
            return Err("The signed-in account changed".into());
        }
        self.call(
            "verifyNotificationFid",
            &[(att.fid as f64).into(), (att.expiry as f64).into(), att.signature.into()],
        )
        .await?;
        self.check()?;
        bounded(self.raw("subscribe", &[QUERY.into()])?).await?;
        self.check()?;
        self.inner.state.borrow_mut().hydrated();
        let state = self.inner.state.borrow();
        self.notifications.synced(self.fid, state.seen(), state.baseline());
        Ok(())
    }

    fn raw(&self, name: &str, args: &[JsValue]) -> Result<JsValue, String> {
        self.check()?;
        let bridge = self.inner.bridge.borrow().clone().ok_or("Not connected")?;
        method(&bridge, name)?.apply(&bridge, &args.iter().collect::<Array>()).map_err(js_error)
    }

    async fn call(&self, name: &str, args: &[JsValue]) -> Result<(), String> {
        parse::<Called>(bounded(self.raw(name, args)?).await?)?.result()
    }

    fn event(&self, json: &str) {
        #[derive(serde::Deserialize)]
        struct Event {
            t: String,
            #[serde(default)]
            table: String,
            row: Option<Row>,
        }
        #[derive(serde::Deserialize)]
        struct Row {
            fid: u64,
            seen_at: i64,
        }
        let Ok(event) = serde_json::from_str::<Event>(json) else { return };
        if event.t == "disconnected" {
            self.inner.disconnected.set(true);
            self.retry();
        } else if event.table == "my_notification_state"
            && matches!(event.t.as_str(), "insert" | "update")
            && let Some(row) = event.row.filter(|r| r.fid == self.fid)
        {
            let mut state = self.inner.state.borrow_mut();
            state.observe_remote(row.seen_at, now());
            self.notifications.synced(self.fid, state.seen(), state.baseline());
        }
    }

    fn flush(&self) {
        if !self.active() || !self.inner.ready.get() || self.inner.writing.replace(true) {
            return;
        }
        let sync = self.clone();
        leptos::task::spawn_local(async move {
            while sync.active() && sync.inner.ready.get() {
                let Some(at) = sync.inner.state.borrow().pending() else { break };
                if sync.call("markNotificationsSeen", &[(at as f64).into()]).await.is_err() {
                    sync.inner.writing.set(false);
                    sync.retry();
                    return;
                }
                if !sync.active() {
                    break;
                }
                sync.inner.state.borrow_mut().acknowledged(at);
            }
            sync.inner.writing.set(false);
            if sync.active() && !sync.inner.ready.get() {
                sync.retry();
            }
        });
    }
}

/// A stalled connection/subscription/reducer cannot hold a retry forever.
async fn bounded(value: JsValue) -> Result<JsValue, String> {
    let timer = RefCell::new(None);
    let timeout = Promise::new(&mut |_, reject| {
        let reject = reject.bind1(&JsValue::NULL, &"Notification sync timed out".into());
        *timer.borrow_mut() = set_timeout(reject.as_ref(), 30_000).ok();
    });
    let answer = await_promise(Promise::race(&Array::of2(&Promise::resolve(&value), &timeout)).into()).await;
    if let Some(timer) = timer.borrow_mut().take() {
        clear_timeout(&timer);
    }
    answer
}

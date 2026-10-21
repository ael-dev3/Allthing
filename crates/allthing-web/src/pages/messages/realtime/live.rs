// SPDX-License-Identifier: AGPL-3.0-or-later
//! The connection to Allthing's SpacetimeDB database, through the bridge
//! (web/stdb/bridge.ts), which this adds to the page as a module script the
//! first time a reader opens Chat or Encrypted: nothing else fetches it.
//!
//! Signed in as `fid`, it connects with the token this browser keeps for
//! the account (`keys::realtime`), or none the first time, and keeps the
//! token the database gives back. It follows the rows Messages shows
//! (`allthing_core::stdb::subscriptions`) into a [`Mirror`], then makes
//! the identity the account's: `register`, the Worker's attestation for
//! `verify_fid`, and this browser's X25519 public key (`set_public_key`).
//! One connection serves both tabs and lasts the visit.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use allthing_core::stdb::{
    AttestRequest, Attestation, Called, Event, Mirror, STDB_DATABASE, STDB_URI, hex, subscriptions,
};
use js_sys::{Function, Promise, Reflect};
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::CryptoKey;

use crate::api::{self, Failure};
use crate::keys::{self, KeyError, Realtime};

/// The bridge's fingerprinted path, from `cargo xtask dist`; a build
/// without it (`cargo build` alone) has no Chat or Encrypted.
const BRIDGE: Option<&str> = option_env!("ALLTHING_STDB_BRIDGE");

/// Where the connection is.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Fetching the bridge, connecting, following the rows.
    Connecting,
    /// Asking the Worker to vouch for the identity, and the database to take it.
    Verifying,
    /// Ready. `verified`: the database verified the account; without
    /// Allthing's attestation key (not set up yet) it can't.
    Ready { verified: bool },
    /// Disconnected, or never connected: why.
    Failed(String),
}

/// The live connection: the bridge, the rows it follows and where it is.
#[derive(Clone)]
pub struct Live {
    pub fid: u64,
    pub phase: ArcRwSignal<Phase>,
    pub mirror: ArcRwSignal<Mirror>,
    /// This browser's identity, once connected.
    pub identity: ArcRwSignal<Option<String>>,
    inner: Arc<SendWrapper<Inner>>,
}

struct Inner {
    bridge: RefCell<Option<JsValue>>,
    /// The X25519 key pair, once made or read.
    key: RefCell<Option<(CryptoKey, [u8; 32])>>,
    /// Shared secrets agreed this visit, by the other public key.
    secrets: RefCell<BTreeMap<[u8; 32], [u8; 32]>>,
    _emit: RefCell<Option<Emit>>,
}

/// The callback the bridge reports rows to.
type Emit = Closure<dyn FnMut(String)>;

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
}

/// The connection for `fid`, started the first time it's asked for this
/// visit (or again for another account).
pub fn live(fid: u64) -> Live {
    if let Some(live) = LIVE.with(|l| l.borrow().clone()).filter(|l| l.fid == fid) {
        return live;
    }
    if let Some(old) = LIVE.with(|l| l.borrow_mut().take()) {
        old.disconnect();
    }
    let live = Live {
        fid,
        phase: ArcRwSignal::new(Phase::Connecting),
        mirror: ArcRwSignal::new(Mirror::default()),
        identity: ArcRwSignal::new(None),
        inner: Arc::new(SendWrapper::new(Inner {
            bridge: RefCell::new(None),
            key: RefCell::new(None),
            secrets: RefCell::new(BTreeMap::new()),
            _emit: RefCell::new(None),
        })),
    };
    LIVE.with(|l| *l.borrow_mut() = Some(live.clone()));
    let started = live.clone();
    leptos::task::spawn_local(async move {
        if let Err(why) = started.start().await {
            started.phase.set(Phase::Failed(why));
        }
    });
    live
}

impl Live {
    async fn start(&self) -> Result<(), String> {
        let path = BRIDGE.ok_or("Messages aren't part of this build")?;
        let mut kept = keys::realtime(self.fid).await.map_err(key_error)?.unwrap_or(Realtime {
            fid: self.fid,
            token: None,
            identity: None,
            key: None,
        });
        let global = load_bridge(path).await?;

        let mirror = self.mirror.clone();
        let phase = self.phase.clone();
        let emit = Closure::<dyn FnMut(String)>::new(move |json: String| match serde_json::from_str::<Event>(&json) {
            Ok(Event::Disconnected { error }) => phase.set(Phase::Failed(
                error.map_or_else(|| "Disconnected from the database".into(), |e| format!("Disconnected: {e}")),
            )),
            Ok(event) => mirror.update(|m| {
                m.apply(&event);
            }),
            Err(_) => {}
        });
        let connect = method(&global, "connect")?;
        let args = js_sys::Array::of4(
            &uri().into(),
            &STDB_DATABASE.into(),
            &kept.token.clone().unwrap_or_default().into(),
            emit.as_ref(),
        );
        let bridge = connect.apply(&global, &args).map_err(js_error)?;
        *self.inner._emit.borrow_mut() = Some(emit);
        *self.inner.bridge.borrow_mut() = Some(bridge.clone());

        #[derive(serde::Deserialize)]
        struct Connected {
            identity: String,
            token: String,
        }
        let connected: Connected =
            parse(await_promise(Reflect::get(&bridge, &"connected".into()).map_err(js_error)?).await?)?;
        if kept.token.as_deref() != Some(connected.token.as_str())
            || kept.identity.as_deref() != Some(connected.identity.as_str())
        {
            kept.token = Some(connected.token);
            kept.identity = Some(connected.identity.clone());
            keys::keep_realtime(&kept).await.map_err(key_error)?;
        }
        self.identity.set(Some(connected.identity.clone()));
        let queries = serde_json::to_string(&subscriptions()).unwrap_or_default();
        await_promise(self.call_raw("subscribe", &[queries.into()])?).await?;

        // The account: claimed, then vouched for by the Worker.
        self.phase.set(Phase::Verifying);
        let me = connected.identity;
        let (claimed, verified) = self.mirror.with(|m| (m.fid_of(&me), m.verified(&me)));
        if claimed != Some(self.fid) {
            // Refused only when the identity verified another account,
            // which a token kept per account can't be.
            let _ = self.call("register", &[(self.fid as f64).into()]).await;
        }
        let mut verified = verified && claimed == Some(self.fid);
        if !verified {
            match api::post::<_, Attestation>("/api/v1/auth/attest", &AttestRequest { identity: me.clone() }).await {
                Ok(att) => {
                    self.call(
                        "verifyFid",
                        &[(att.fid as f64).into(), (att.expiry as f64).into(), att.signature.into()],
                    )
                    .await?;
                    verified = true;
                }
                // The Worker has no attestation key yet: the module, with
                // none pinned, lets the claimed account speak, unverified.
                Err(Failure::NotFound(_)) => {}
                Err(failure) => return Err(failure.message()),
            }
        }

        // This browser's message key, its public half published.
        let pair = match kept.key.clone() {
            Some(pair) => pair,
            None => {
                let pair = keys::x25519_generate().await.map_err(key_error)?;
                kept.key = Some(pair.clone());
                keys::keep_realtime(&kept).await.map_err(key_error)?;
                pair
            }
        };
        let public = hex(&pair.1);
        *self.inner.key.borrow_mut() = Some(pair);
        let published = self.mirror.with(|m| m.keys.get(&me).map(|k| k.x25519_pub.clone()));
        if published.as_deref() != Some(public.as_str()) {
            self.call("setPublicKey", &[public.into()]).await?;
        }
        self.phase.set(Phase::Ready { verified });
        Ok(())
    }

    fn call_raw(&self, name: &str, args: &[JsValue]) -> Result<JsValue, String> {
        let bridge = self.inner.bridge.borrow().clone().ok_or("Not connected")?;
        let f = method(&bridge, name)?;
        f.apply(&bridge, &args.iter().collect::<js_sys::Array>()).map_err(js_error)
    }

    /// Call the reducer `name` (the bridge's camel-case name), and say
    /// whether the database took it.
    pub async fn call(&self, name: &str, args: &[JsValue]) -> Result<(), String> {
        let answer = await_promise(self.call_raw(name, args)?).await?;
        parse::<Called>(answer)?.result()
    }

    /// The X25519 secret this browser's key agrees with `peer`'s public key.
    pub async fn secret(&self, peer: &[u8; 32]) -> Result<[u8; 32], String> {
        if let Some(secret) = self.inner.secrets.borrow().get(peer) {
            return Ok(*secret);
        }
        let private = self.inner.key.borrow().as_ref().map(|(k, _)| k.clone()).ok_or("No message key yet")?;
        let secret = keys::x25519_shared(&private, peer).await.map_err(key_error)?;
        self.inner.secrets.borrow_mut().insert(*peer, secret);
        Ok(secret)
    }

    /// This browser's public key, once it has one.
    pub fn public_key(&self) -> Option<[u8; 32]> {
        self.inner.key.borrow().as_ref().map(|(_, p)| *p)
    }

    fn disconnect(&self) {
        let _ = self.call_raw("disconnect", &[]);
    }
}

/// The database's host: maincloud, except on a development server on this
/// machine, where `localStorage["allthing-stdb-uri"]` may name a local
/// SpacetimeDB (`ws://127.0.0.1:<port>`) to test against.
fn uri() -> String {
    let local = web_sys::window().and_then(|w| {
        let host = w.location().hostname().ok()?;
        if host != "127.0.0.1" && host != "localhost" {
            return None;
        }
        let uri = w.local_storage().ok()??.get_item("allthing-stdb-uri").ok()??;
        (uri.starts_with("ws://127.0.0.1:") || uri.starts_with("ws://localhost:")).then_some(uri)
    });
    local.unwrap_or_else(|| STDB_URI.to_owned())
}

fn key_error(error: KeyError) -> String {
    match error {
        KeyError::Unsupported => {
            "This browser can't keep message keys (it needs WebCrypto's X25519 and IndexedDB)".into()
        }
        KeyError::Failed(why) => why,
    }
}

fn js_error(error: JsValue) -> String {
    Reflect::get(&error, &"message".into())
        .ok()
        .and_then(|m| m.as_string())
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "Something went wrong".into())
}

fn method(object: &JsValue, name: &str) -> Result<Function, String> {
    Reflect::get(object, &name.into())
        .ok()
        .and_then(|f| f.dyn_into::<Function>().ok())
        .ok_or_else(|| format!("The database bridge has no {name}"))
}

async fn await_promise(value: JsValue) -> Result<JsValue, String> {
    match value.dyn_into::<Promise>() {
        Ok(promise) => JsFuture::from(promise).await.map_err(js_error),
        Err(value) => Ok(value),
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: JsValue) -> Result<T, String> {
    serde_json::from_str(&value.as_string().unwrap_or_default()).map_err(|_| "The database answered oddly".into())
}

/// The bridge's `window.allthingStdb`, its script added the first time.
async fn load_bridge(path: &str) -> Result<JsValue, String> {
    let window = web_sys::window().ok_or("No window")?;
    let present = |w: &web_sys::Window| Reflect::get(w, &"allthingStdb".into()).ok().filter(|v| v.is_object());
    if let Some(global) = present(&window) {
        return Ok(global);
    }
    let document = window.document().ok_or("No document")?;
    let src = format!("{}{path}", crate::util::BASE);
    let (script, added) = match document.query_selector("script[data-stdb]").ok().flatten() {
        Some(script) => (script, true),
        None => {
            let script = document.create_element("script").map_err(js_error)?;
            let _ = script.set_attribute("type", "module");
            let _ = script.set_attribute("data-stdb", "");
            let _ = script.set_attribute("src", &src);
            (script, false)
        }
    };
    let loaded = Promise::new(&mut |resolve, reject| {
        let ok = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });
        let failed = Closure::once_into_js(move || {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("Couldn't load Messages; check your connection"));
        });
        let _ = script.add_event_listener_with_callback("load", ok.unchecked_ref());
        let _ = script.add_event_listener_with_callback("error", failed.unchecked_ref());
    });
    if !added {
        let head = document.query_selector("head").ok().flatten().ok_or("No head")?;
        head.append_child(&script).map_err(js_error)?;
    }
    if present(&window).is_none() {
        JsFuture::from(loaded).await.map_err(js_error)?;
    }
    present(&window).ok_or_else(|| "Messages didn't load".into())
}

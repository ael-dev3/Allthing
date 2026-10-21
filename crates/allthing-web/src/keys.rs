// SPDX-License-Identifier: AGPL-3.0-or-later
//! The reader's app key in this browser (see `allthing_core::app_key`): an
//! Ed25519 key pair that WebCrypto makes unexportable, kept in IndexedDB,
//! one per signed-in account. IndexedDB keeps the browser's own key object,
//! which stays unexportable there, so the private key never exists as bytes
//! that a script could read. It signs the reader's messages
//! (`allthing_core::message`); its public half is what the reader approves
//! in the Farcaster app. Not in the static copy, which has no sign-in.

use std::cell::RefCell;

use allthing_core::provenance::hex_encode;
use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{CryptoKey, IdbDatabase, IdbRequest, IdbTransactionMode};

const DATABASE: &str = "allthing-keys";
const STORE: &str = "keys";
/// The account a key made to sign in with is kept for, until the account
/// that approves it is known.
pub const SIGNING_IN: u64 = 0;

/// A key as this browser keeps it.
#[derive(Clone)]
pub struct Stored {
    /// The account it is for.
    pub fid: u64,
    /// Its public half, `0x…`.
    pub key: String,
    private: CryptoKey,
    /// While the Farcaster app hasn't added it: the request to approve,
    /// its link, and when its endorsement lapses (Unix seconds).
    pub request: Option<Request>,
    /// When the reader approved it (ms), once they have.
    pub approved_at: Option<f64>,
}

impl Stored {
    /// The same key, approved and added to the account just now.
    pub fn approved(self) -> Stored {
        Stored { request: None, approved_at: Some(js_sys::Date::now()), ..self }
    }

    /// The same key, kept for `fid`: the account that approved a key made
    /// to sign in with.
    pub fn for_account(self, fid: u64) -> Stored {
        Stored { fid, ..self }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub token: String,
    pub link: String,
    pub deadline: f64,
}

/// Why there is no key.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyError {
    /// This browser's WebCrypto has no Ed25519, or no IndexedDB.
    Unsupported,
    Failed(String),
}

thread_local! {
    static OPENED: RefCell<Option<IdbDatabase>> = const { RefCell::new(None) };
}

/// A new key for `fid`, kept by nothing yet.
pub async fn generate(fid: u64) -> Result<Stored, KeyError> {
    let subtle = subtle().ok_or(KeyError::Unsupported)?;
    let usages = Array::of2(&JsValue::from_str("sign"), &JsValue::from_str("verify"));
    let pair = subtle.generate_key_with_object(&ed25519(), false, &usages).map_err(|_| KeyError::Unsupported)?;
    let pair = JsFuture::from(pair).await.map_err(|_| KeyError::Unsupported)?;
    let private: CryptoKey = field(&pair, "privateKey").dyn_into().map_err(|_| KeyError::Unsupported)?;
    let public: CryptoKey = field(&pair, "publicKey").dyn_into().map_err(|_| KeyError::Unsupported)?;
    let raw = subtle.export_key("raw", &public).map_err(|_| KeyError::Unsupported)?;
    let raw = Uint8Array::new(&JsFuture::from(raw).await.map_err(|_| KeyError::Unsupported)?).to_vec();
    if raw.len() != 32 {
        return Err(KeyError::Unsupported);
    }
    Ok(Stored { fid, key: hex_encode(&raw), private, request: None, approved_at: None })
}

/// The 64-byte Ed25519 signature of `bytes` by the stored key.
pub async fn sign(stored: &Stored, bytes: &[u8]) -> Result<[u8; 64], KeyError> {
    let subtle = subtle().ok_or(KeyError::Unsupported)?;
    let signed = subtle.sign_with_str_and_u8_array("Ed25519", &stored.private, bytes).map_err(failed)?;
    let signature = Uint8Array::new(&JsFuture::from(signed).await.map_err(failed)?).to_vec();
    signature.try_into().map_err(|_| KeyError::Failed("The signature isn't 64 bytes".into()))
}

/// The key this browser keeps for `fid`, if any.
pub async fn load(fid: u64) -> Result<Option<Stored>, KeyError> {
    let found = request(IdbTransactionMode::Readonly, |store| store.get(&JsValue::from_f64(fid as f64))).await?;
    if found.is_undefined() || found.is_null() {
        return Ok(None);
    }
    let private =
        field(&found, "private").dyn_into::<CryptoKey>().map_err(|_| KeyError::Failed("Unreadable key".into()))?;
    let text = |name: &str| field(&found, name).as_string();
    let number = |name: &str| field(&found, name).as_f64();
    let request = match (text("token"), text("link"), number("deadline")) {
        (Some(token), Some(link), Some(deadline)) => Some(Request { token, link, deadline }),
        _ => None,
    };
    Ok(Some(Stored { fid, key: text("key").unwrap_or_default(), private, request, approved_at: number("approvedAt") }))
}

/// Keep `stored`, in place of any key kept for its account.
pub async fn save(stored: &Stored) -> Result<(), KeyError> {
    let record = Object::new();
    let set = |name: &str, value: &JsValue| {
        let _ = Reflect::set(&record, &JsValue::from_str(name), value);
    };
    set("fid", &JsValue::from_f64(stored.fid as f64));
    set("key", &JsValue::from_str(&stored.key));
    set("private", &stored.private);
    if let Some(request) = &stored.request {
        set("token", &JsValue::from_str(&request.token));
        set("link", &JsValue::from_str(&request.link));
        set("deadline", &JsValue::from_f64(request.deadline));
    }
    if let Some(at) = stored.approved_at {
        set("approvedAt", &JsValue::from_f64(at));
    }
    request(IdbTransactionMode::Readwrite, |store| store.put(&record)).await.map(|_| ())
}

/// Forget the key kept for `fid`.
pub async fn remove(fid: u64) -> Result<(), KeyError> {
    request(IdbTransactionMode::Readwrite, |store| store.delete(&JsValue::from_f64(fid as f64))).await.map(|_| ())
}

fn subtle() -> Option<web_sys::SubtleCrypto> {
    Some(web_sys::window()?.crypto().ok()?.subtle())
}

fn ed25519() -> Object {
    let algorithm = Object::new();
    let _ = Reflect::set(&algorithm, &JsValue::from_str("name"), &JsValue::from_str("Ed25519"));
    algorithm
}

fn field(object: &JsValue, name: &str) -> JsValue {
    Reflect::get(object, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}

fn failed(error: JsValue) -> KeyError {
    KeyError::Failed(field(&error, "message").as_string().unwrap_or_else(|| "WebCrypto failed".into()))
}

/// One request on the key store, in a transaction of its own.
async fn request(
    mode: IdbTransactionMode,
    make: impl FnOnce(&web_sys::IdbObjectStore) -> Result<IdbRequest, JsValue>,
) -> Result<JsValue, KeyError> {
    let database = database().await?;
    let transaction = database.transaction_with_str_and_mode(STORE, mode).map_err(failed)?;
    let store = transaction.object_store(STORE).map_err(failed)?;
    finished(&make(&store).map_err(failed)?).await
}

/// The key database, opened once per page (and made on the first visit).
async fn database() -> Result<IdbDatabase, KeyError> {
    if let Some(database) = OPENED.with(|o| o.borrow().clone()) {
        return Ok(database);
    }
    let factory = web_sys::window().and_then(|w| w.indexed_db().ok().flatten()).ok_or(KeyError::Unsupported)?;
    let opening = factory.open_with_u32(DATABASE, 1).map_err(|_| KeyError::Unsupported)?;
    let upgrade = opening.clone();
    let make_store = Closure::once_into_js(move || {
        let Ok(database) = upgrade.result().and_then(|r| r.dyn_into::<IdbDatabase>()) else {
            return;
        };
        let parameters = web_sys::IdbObjectStoreParameters::new();
        parameters.set_key_path(&JsValue::from_str("fid"));
        let _ = database.create_object_store_with_optional_parameters(STORE, &parameters);
    });
    opening.set_onupgradeneeded(Some(make_store.unchecked_ref()));
    let database: IdbDatabase = finished(&opening).await?.dyn_into().map_err(|_| KeyError::Unsupported)?;
    OPENED.with(|o| *o.borrow_mut() = Some(database.clone()));
    Ok(database)
}

/// An IndexedDB request's result, once it has one.
async fn finished(request: &IdbRequest) -> Result<JsValue, KeyError> {
    let promise = Promise::new(&mut |resolve, reject| {
        let done = request.clone();
        let success = Closure::once_into_js(move || {
            let _ = resolve.call1(&JsValue::NULL, &done.result().unwrap_or(JsValue::UNDEFINED));
        });
        let failure = Closure::once_into_js(move || {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("IndexedDB failed"));
        });
        request.set_onsuccess(Some(success.unchecked_ref()));
        request.set_onerror(Some(failure.unchecked_ref()));
    });
    JsFuture::from(promise).await.map_err(|_| KeyError::Failed("This browser couldn't keep the key".into()))
}

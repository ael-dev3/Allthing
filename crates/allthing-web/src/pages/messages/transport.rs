// SPDX-License-Identifier: AGPL-3.0-or-later
//! How the Messages page reaches the reader's Direct Casts
//! (`allthing_core::dc`, docs/DIRECT_CASTS.md), with the reader's own API
//! key. Three ways, tried in this order:
//!
//! 1. **direct**: this browser calls the Direct Cast API itself. It does
//!    only where the API answers this site's origin (CORS), which a cheap
//!    read finds out ([`Call::probe`]). Where it doesn't, the browser never
//!    sends the key: its preflight is refused first.
//! 2. **relay**: the Worker's `/api/v1/dc/…` passes each call on with the
//!    key, where its owner has turned that on (`GET /api/v1/dc` says so).
//! 3. **none**: nothing reads. A message can still be sent with
//!    Programmable DCs' `PUT /v2/ext-send-direct-cast`, which answers any
//!    origin, from this browser ([`ext_send`]).

use std::cell::RefCell;

use allthing_core::dc::{self, API, Call, EXT_SEND_PATH, ExtSend, ExtSent, Page, Refusal};
use serde::de::{DeserializeOwned, IgnoredAny};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// How long a call may take before it is given up.
const TIMEOUT_MS: i32 = 12_000;

/// How this browser reads the reader's Direct Casts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Direct,
    Relay,
    /// Nothing reads them: sending only.
    None,
}

/// Why a call didn't do what it was asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// The API refused it, and says why (its status, and its message).
    Refused(Refusal, Option<String>),
    /// The relay refused it itself (turned off, no session, too many
    /// calls): its status, and its message.
    Relay(u16, String),
    /// No answer, or none this page may read (CORS).
    Unreachable,
    /// An answer that isn't what the API sends.
    Unreadable,
}

impl Failure {
    /// What to tell the reader, about a message to `to` ("@alice") when it
    /// was one.
    pub fn message(&self, to: Option<&str>) -> String {
        match self {
            Failure::Refused(refusal, _) => refusal.message(to),
            Failure::Relay(_, why) => why.clone(),
            Failure::Unreachable => "Couldn't reach Farcaster's API. Check the connection and try again.".into(),
            Failure::Unreadable => "Farcaster's API answered in a way Allthing doesn't understand.".into(),
        }
    }

    /// Farcaster turned the key away: it is wrong, expired or revoked.
    pub fn key_refused(&self) -> bool {
        matches!(self, Failure::Refused(Refusal::Key, _))
    }
}

thread_local! {
    /// The way found for a key this visit: found once.
    static FOUND: RefCell<Option<(String, Backend)>> = const { RefCell::new(None) };
}

/// How this browser can read the reader's Direct Casts with `key`: an
/// error only when Farcaster turned the key away.
pub async fn detect(key: &str) -> Result<Backend, Failure> {
    let found = FOUND.with(|f| f.borrow().as_ref().filter(|(k, _)| k == key).map(|(_, backend)| *backend));
    if let Some(backend) = found {
        return Ok(backend);
    }
    let backend = find(key).await?;
    FOUND.with(|f| *f.borrow_mut() = Some((key.to_owned(), backend)));
    Ok(backend)
}

async fn find(key: &str) -> Result<Backend, Failure> {
    match call::<IgnoredAny>(Backend::Direct, key, &Call::probe(), None).await {
        Ok(_) => return Ok(Backend::Direct),
        Err(Failure::Unreachable) => {}
        Err(failure) if failure.key_refused() => return Err(failure),
        // It answers this origin: reading goes there, even while it's down.
        Err(_) => return Ok(Backend::Direct),
    }
    if !relay_enabled().await {
        return Ok(Backend::None);
    }
    match call::<IgnoredAny>(Backend::Relay, key, &Call::probe(), None).await {
        Ok(_) => Ok(Backend::Relay),
        Err(failure) if failure.key_refused() => Err(failure),
        // Turned off meanwhile.
        Err(Failure::Relay(404, _)) => Ok(Backend::None),
        // On, but refusing or failing just now: it reads, and says why.
        Err(_) => Ok(Backend::Relay),
    }
}

/// Whether the Worker relays the Direct Cast API (its owner turned it on).
pub async fn relay_enabled() -> bool {
    matches!(exchange("GET", &dc::relay_path(), &[], None).await, Ok((200, _, _)))
}

/// Make `call` with `key`, the way `backend` reads, and read its answer.
/// A send takes an idempotency key, so a retry sends nothing twice.
pub async fn call<T: DeserializeOwned>(
    backend: Backend,
    key: &str,
    call: &Call,
    idempotency: Option<&str>,
) -> Result<T, Failure> {
    let bearer = format!("Bearer {key}");
    let (url, mut headers) = match backend {
        Backend::Direct => (call.url(API), vec![("authorization", bearer.as_str())]),
        Backend::Relay => (call.relay_url(), vec![(dc::KEY_HEADER, key)]),
        Backend::None => return Err(Failure::Unreachable),
    };
    if let Some(idempotency) = idempotency {
        headers.push(("idempotency-key", idempotency));
    }
    let (status, text, upstream) = exchange(call.method().as_str(), &url, &headers, call.body.as_deref())
        .await
        .map_err(|_| Failure::Unreachable)?;
    if backend == Backend::Relay && !upstream && !(200..300).contains(&status) {
        let why = serde_json::from_str::<allthing_core::api::ApiError>(&text).map(|e| e.error);
        return Err(Failure::Relay(
            status,
            why.unwrap_or_else(|_| format!("Allthing's relay answered HTTP {status}.")),
        ));
    }
    read(status, &text)
}

/// Send `text` to `fid` with Programmable DCs, from this browser: the one
/// endpoint that answers any origin.
pub async fn ext_send(key: &str, fid: u64, text: &str, idempotency: &str) -> Result<(), Failure> {
    let send = ExtSend { recipient_fid: fid, message: text.to_owned(), idempotency_key: idempotency.to_owned() };
    let body = serde_json::to_string(&send).map_err(|_| Failure::Unreadable)?;
    let bearer = format!("Bearer {key}");
    let url = format!("{API}{EXT_SEND_PATH}");
    let (status, text, _) =
        exchange("PUT", &url, &[("authorization", &bearer)], Some(&body)).await.map_err(|_| Failure::Unreachable)?;
    let sent: Page<ExtSent> = read(status, &text)?;
    if sent.result.success { Ok(()) } else { Err(Failure::Unreadable) }
}

fn read<T: DeserializeOwned>(status: u16, text: &str) -> Result<T, Failure> {
    match Refusal::from_status(status) {
        None => serde_json::from_str(text).map_err(|_| Failure::Unreadable),
        Some(refusal) => Err(Failure::Refused(refusal, dc::error_message(text))),
    }
}

/// One request: its status, body, and whether the relay marked it the API's
/// own answer. `Err` when it got no answer this page may read.
async fn exchange(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Result<(u16, String, bool), ()> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    // Private: never kept in the browser's HTTP cache.
    init.set_cache(web_sys::RequestCache::NoStore);
    let sent = web_sys::Headers::new().map_err(|_| ())?;
    for (name, value) in headers {
        sent.set(name, value).map_err(|_| ())?;
    }
    if let Some(body) = body {
        sent.set("content-type", "application/json").map_err(|_| ())?;
        init.set_body(&JsValue::from_str(body));
    }
    init.set_headers(&sent);
    let window = web_sys::window().ok_or(())?;
    if let Ok(abort) = web_sys::AbortController::new() {
        init.set_signal(Some(&abort.signal()));
        let stop = Closure::once_into_js(move || abort.abort());
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(stop.unchecked_ref(), TIMEOUT_MS);
    }
    let response = JsFuture::from(window.fetch_with_str_and_init(url, &init)).await.map_err(|_| ())?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| ())?;
    let upstream = response.headers().get(dc::RELAY_HEADER).ok().flatten().as_deref() == Some("upstream");
    let text = JsFuture::from(response.text().map_err(|_| ())?).await.map_err(|_| ())?;
    Ok((response.status(), text.as_string().unwrap_or_default(), upstream))
}

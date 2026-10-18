// SPDX-License-Identifier: AGPL-3.0-or-later
//! Same-origin calls to the Worker's `/api/v1`, with a short in-memory cache
//! so going back and forth between pages costs no requests. In the static
//! build (`static-host`), the same payloads are built in the browser.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use allthing_core::api::ApiError;
use serde::de::DeserializeOwned;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// How long a fetched payload is reused without asking the Worker again.
/// Long enough that going back to a feed or post is instant.
const CLIENT_TTL_MS: f64 = 300_000.0;
/// At most this many prefetches in flight.
const MAX_PREFETCHES: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    NotFound(String),
    /// The Worker or an upstream is shedding load; retry later.
    Busy {
        message: String,
        retry_after: Option<u32>,
    },
    Failed(String),
}

impl Failure {
    pub fn message(&self) -> String {
        match self {
            Failure::NotFound(m) | Failure::Failed(m) => m.clone(),
            Failure::Busy { message, retry_after } => match retry_after {
                Some(s) => format!("{message} (try again in {s}s)"),
                None => message.clone(),
            },
        }
    }
}

#[derive(Debug)]
pub struct Fetched<T> {
    pub data: Arc<T>,
    /// Seconds since the Worker fetched the data upstream.
    pub age: u32,
    /// Served from cache because the upstream is failing.
    pub stale: bool,
}

impl<T> Clone for Fetched<T> {
    fn clone(&self) -> Self {
        Fetched { data: self.data.clone(), age: self.age, stale: self.stale }
    }
}

impl<T> PartialEq for Fetched<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
    }
}

struct Raw {
    at: f64,
    status: u16,
    body: Rc<str>,
    age: u32,
    stale: bool,
    retry_after: Option<u32>,
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Raw>> = RefCell::new(HashMap::new());
    static IN_FLIGHT: RefCell<std::collections::HashSet<String>> = RefCell::new(std::collections::HashSet::new());
    /// Casts already shown in a feed, so their post page can render at once.
    static SEEN_CASTS: RefCell<HashMap<String, (allthing_core::model::Cast, Option<u32>)>> = RefCell::new(HashMap::new());
}

/// Remember the casts a listing shows (with their authors' scores).
pub fn remember_casts(casts: &[allthing_core::model::Cast], notes: &allthing_core::api::Annotations) {
    SEEN_CASTS.with(|seen| {
        let mut seen = seen.borrow_mut();
        if seen.len() > 2000 {
            seen.clear();
        }
        for cast in casts {
            seen.insert(cast.hash.clone(), (cast.clone(), notes.scores.get(&cast.author.fid).copied()));
        }
    });
}

/// A cast seen in a listing, for rendering its post before the thread loads.
pub fn seen_cast(hash: &str) -> Option<(allthing_core::model::Cast, Option<u32>)> {
    SEEN_CASTS.with(|seen| seen.borrow().get(hash).cloned())
}

/// Whether the reader asked to save data or is on a slow connection.
pub fn constrained_network() -> bool {
    use wasm_bindgen::JsValue;
    let Some(window) = web_sys::window() else { return true };
    let Ok(connection) = js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("connection")) else {
        return false;
    };
    if connection.is_undefined() {
        return false;
    }
    let get = |key: &str| js_sys::Reflect::get(&connection, &JsValue::from_str(key)).ok();
    let save_data = get("saveData").and_then(|v| v.as_bool()).unwrap_or(false);
    let slow = get("effectiveType").and_then(|v| v.as_string()).is_some_and(|t| t.contains("2g"));
    save_data || slow
}

/// Fetch `path` into the client cache in the background, so opening it later
/// is instant. Skipped when already cached or loading, when too many
/// prefetches are running, and on Save-Data or slow connections.
pub fn prefetch(path: &str) {
    let now = js_sys::Date::now();
    let cached = CACHE.with(|c| c.borrow().get(path).is_some_and(|r| now - r.at < CLIENT_TTL_MS && r.status == 200));
    if cached || constrained_network() {
        return;
    }
    let started = IN_FLIGHT.with(|f| {
        let mut f = f.borrow_mut();
        if f.len() >= MAX_PREFETCHES || f.contains(path) { false } else { f.insert(path.to_owned()) }
    });
    if !started {
        return;
    }
    let path = path.to_owned();
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(raw) = fetch_raw(&path).await {
            CACHE.with(|c| {
                c.borrow_mut().insert(path.clone(), raw);
            });
        }
        IN_FLIGHT.with(|f| f.borrow_mut().remove(&path));
    });
}

async fn fetch_raw(path: &str) -> Result<Raw, Failure> {
    #[cfg(feature = "static-host")]
    if path.starts_with(allthing_core::api::API_PREFIX) {
        let (status, body, retry_after) = crate::host::build(path).await;
        return Ok(Raw { at: js_sys::Date::now(), status, body: body.into(), age: 0, stale: false, retry_after });
    }
    let window = web_sys::window().ok_or_else(|| Failure::Failed("no window".into()))?;
    let response = JsFuture::from(window.fetch_with_str(path))
        .await
        .map_err(|_| Failure::Failed("Network error: could not reach Allthing's API".into()))?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| Failure::Failed("bad response".into()))?;
    let header = |name: &str| response.headers().get(name).ok().flatten();
    let age = header("x-allthing-age").and_then(|a| a.parse().ok()).unwrap_or(0);
    let stale = header("x-allthing-stale").is_some();
    let retry_after = header("retry-after").and_then(|a| a.parse().ok());
    let text = JsFuture::from(response.text().map_err(|_| Failure::Failed("unreadable response".into()))?)
        .await
        .map_err(|_| Failure::Failed("unreadable response".into()))?
        .as_string()
        .unwrap_or_default();
    Ok(Raw { at: js_sys::Date::now(), status: response.status(), body: text.into(), age, stale, retry_after })
}

fn decode<T: DeserializeOwned>(raw: &Raw) -> Result<Fetched<T>, Failure> {
    let error_message = || {
        serde_json::from_str::<ApiError>(&raw.body).map(|e| e.error).unwrap_or_else(|_| format!("HTTP {}", raw.status))
    };
    match raw.status {
        200 => serde_json::from_str::<T>(&raw.body)
            .map(|data| Fetched { data: Arc::new(data), age: raw.age, stale: raw.stale })
            .map_err(|e| Failure::Failed(format!("Unexpected data from the API: {e}"))),
        404 => Err(Failure::NotFound(error_message())),
        429 | 503 => Err(Failure::Busy { message: error_message(), retry_after: raw.retry_after }),
        _ => Err(Failure::Failed(error_message())),
    }
}

/// GET an API path, reusing a recent answer for the same path.
pub async fn get<T: DeserializeOwned>(path: &str) -> Result<Fetched<T>, Failure> {
    // A prefetch of the same path is on its way: wait for it, don't repeat it.
    for _ in 0..400 {
        if !IN_FLIGHT.with(|f| f.borrow().contains(path)) {
            break;
        }
        crate::video::sleep(25).await;
    }
    let now = js_sys::Date::now();
    let cached = CACHE.with(|c| {
        c.borrow().get(path).filter(|raw| now - raw.at < CLIENT_TTL_MS && raw.status != 503 && raw.status != 429).map(
            |raw| {
                let age = raw.age + ((now - raw.at) / 1000.0) as u32;
                decode::<T>(&Raw {
                    at: raw.at,
                    status: raw.status,
                    body: raw.body.clone(),
                    age,
                    stale: raw.stale,
                    retry_after: raw.retry_after,
                })
            },
        )
    });
    if let Some(result) = cached {
        return result;
    }
    let raw = fetch_raw(path).await?;
    let result = decode::<T>(&raw);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 200 {
            c.retain(|_, r| now - r.at < CLIENT_TTL_MS);
        }
        c.insert(path.to_owned(), raw);
    });
    result
}

/// GET a static text asset (label shards), cached for the session.
pub async fn get_text(path: &str) -> Option<Rc<str>> {
    if let Some(body) = CACHE.with(|c| c.borrow().get(path).filter(|r| r.status == 200).map(|r| r.body.clone())) {
        return Some(body);
    }
    let raw = fetch_raw(path).await.ok()?;
    let body = (raw.status == 200).then(|| raw.body.clone());
    CACHE.with(|c| c.borrow_mut().insert(path.to_owned(), raw));
    body
}

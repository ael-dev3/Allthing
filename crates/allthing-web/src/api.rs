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
const CLIENT_TTL_MS: f64 = 60_000.0;

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
        if c.len() > 64 {
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

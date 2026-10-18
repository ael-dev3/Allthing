// SPDX-License-Identifier: AGPL-3.0-or-later
//! Static hosting (the GitHub Pages build): there is no Worker, so the browser
//! builds each page's payload itself with the Worker's own builders, reading
//! Hypersnap and the public RPCs directly. All of them allow cross-origin
//! reads; nothing is proxied and nothing is cached beyond this visit.

use std::rc::Rc;

use allthing_core::api::ApiError;
use allthing_data::{Endpoint, FetchError, Host, HttpMethod, HttpRequest, HttpResponse, Invocation};
use futures::future::{Either, FutureExt, LocalBoxFuture, select};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Upstream calls one page may make. Lower than the Worker's budget: the
/// reader's browser makes them, and memoized annotations fill in over a visit.
const BUDGET: u32 = 30;

struct BrowserHost;

async fn exchange(request: HttpRequest) -> Result<HttpResponse, FetchError> {
    let failed = |_: JsValue| FetchError::Network("request failed".into());
    let window = web_sys::window().ok_or_else(|| FetchError::Network("no window".into()))?;
    let init = web_sys::RequestInit::new();
    init.set_method(match request.method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
    });
    let headers = web_sys::Headers::new().map_err(failed)?;
    let _ = headers.set("accept", request.accept);
    if let Some(body) = &request.body {
        let _ = headers.set("content-type", "application/json");
        init.set_body(&JsValue::from_str(body));
    }
    init.set_headers(&headers);
    let controller = web_sys::AbortController::new().map_err(failed)?;
    init.set_signal(Some(&controller.signal()));
    let outgoing = web_sys::Request::new_with_str_and_init(&request.url, &init).map_err(failed)?;

    let send = async {
        let response: web_sys::Response =
            JsFuture::from(window.fetch_with_request(&outgoing)).await.map_err(failed)?.dyn_into().map_err(failed)?;
        let status = response.status();
        let retry_after = response.headers().get("retry-after").ok().flatten();
        let body = if response.ok() {
            let buffer = JsFuture::from(response.array_buffer().map_err(failed)?).await.map_err(failed)?;
            let mut bytes = js_sys::Uint8Array::new(&buffer).to_vec();
            if let Some(max) = request.max_bytes {
                bytes.truncate(max as usize);
            }
            Some(bytes)
        } else {
            None
        };
        Ok(HttpResponse { status, retry_after, body })
    };
    let timeout = crate::video::sleep(i32::try_from(request.timeout_ms).unwrap_or(i32::MAX));
    futures::pin_mut!(send, timeout);
    match select(send, timeout).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => {
            controller.abort();
            Err(FetchError::Timeout)
        }
    }
}

impl Host for BrowserHost {
    fn now(&self) -> f64 {
        js_sys::Date::now()
    }

    fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
        exchange(request).boxed_local()
    }

    fn spawn(&self, future: LocalBoxFuture<'static, ()>) {
        wasm_bindgen_futures::spawn_local(future);
    }
}

/// Build the payload for an API path in the browser, answering like the
/// Worker would: `(status, JSON body, retry_after)`.
pub async fn build(path: &str) -> (u16, String, Option<u32>) {
    let error = |status: u16, message: String, retry_after: Option<u32>| {
        let body = serde_json::to_string(&ApiError { error: message, retry_after }).unwrap_or_default();
        (status, body, retry_after)
    };
    let endpoint = match Endpoint::from_path(path) {
        Ok(endpoint) => endpoint,
        Err(bad) => return error(bad.status, bad.message.into(), None),
    };
    let inv = Rc::new(Invocation::new(Rc::new(BrowserHost), BUDGET));
    match endpoint.build(inv, None).await {
        Ok(built) => (built.status, String::from_utf8(built.body).unwrap_or_default(), None),
        Err(failure) => error(failure.status, failure.message, failure.retry_after),
    }
}

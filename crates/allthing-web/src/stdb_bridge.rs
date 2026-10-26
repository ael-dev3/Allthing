// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared access to the optional SpacetimeDB SDK bridge. Concurrent callers
//! reuse one module script; each owns and cleans up its own load listeners.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use allthing_core::stdb::STDB_URI;
use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

const LOAD_TIMEOUT_MS: i32 = 30_000;
const WAITERS: &str = "data-stdb-waiters";
const LOAD_FAILED: &str = "Couldn't load Messages; check your connection";
const LOAD_TIMEOUT: &str = "The database bridge took too long to load; please try again";

/// Maincloud, except for an explicitly configured local development database.
pub(crate) fn uri() -> String {
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

pub(crate) fn js_error(error: JsValue) -> String {
    Reflect::get(&error, &"message".into())
        .ok()
        .and_then(|m| m.as_string())
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "Something went wrong".into())
}

pub(crate) fn method(object: &JsValue, name: &str) -> Result<Function, String> {
    Reflect::get(object, &name.into())
        .ok()
        .and_then(|f| f.dyn_into::<Function>().ok())
        .ok_or_else(|| format!("The database bridge has no {name}"))
}

/// Keep the host's timer handle intact. Native browsers use integer IDs;
/// alternate hosts can return wider numbers or opaque handles. Narrowing to
/// web-sys's i32 would leave an owned callback live after its Rust owner drops.
pub(crate) fn set_timeout(callback: &JsValue, millis: i32) -> Result<JsValue, String> {
    let window = web_sys::window().ok_or("No window")?;
    method(window.as_ref(), "setTimeout")?.call2(window.as_ref(), callback, &millis.into()).map_err(js_error)
}

pub(crate) fn clear_timeout(handle: &JsValue) {
    if let Some(window) = web_sys::window()
        && let Ok(clear) = method(window.as_ref(), "clearTimeout")
    {
        let _ = clear.call1(window.as_ref(), handle);
    }
}

/// Await an SDK promise, also accepting bridge methods that return a value.
/// Operation-specific deadlines belong to the caller; this preserves Messages'
/// existing reducer/connection behavior.
pub(crate) async fn await_promise(value: JsValue) -> Result<JsValue, String> {
    match value.dyn_into::<Promise>() {
        Ok(promise) => JsFuture::from(promise).await.map_err(js_error),
        Err(value) => Ok(value),
    }
}

pub(crate) fn parse<T: serde::de::DeserializeOwned>(value: JsValue) -> Result<T, String> {
    serde_json::from_str(&value.as_string().unwrap_or_default()).map_err(|_| "The database answered oddly".into())
}

fn present(window: &web_sys::Window) -> Option<JsValue> {
    Reflect::get(window, &"allthingStdb".into()).ok().filter(|v| v.is_object())
}

/// Load `window.allthingStdb` once. There is no await between looking up the
/// script and appending it, so Messages and notification sync join the same
/// `script[data-stdb]`. A failed load removes its tag so a later call can retry.
pub(crate) async fn load_bridge(path: &str) -> Result<JsValue, String> {
    let window = web_sys::window().ok_or("No window")?;
    if let Some(global) = present(&window) {
        return Ok(global);
    }
    let document = window.document().ok_or("No document")?;
    let (script, added) = match document.query_selector("script[data-stdb]").map_err(js_error)? {
        Some(script) => (script, true),
        None => {
            let script = document.create_element("script").map_err(js_error)?;
            script.set_attribute("type", "module").map_err(js_error)?;
            script.set_attribute("data-stdb", "").map_err(js_error)?;
            script.set_attribute("src", &format!("{}{path}", crate::util::BASE)).map_err(js_error)?;
            (script, false)
        }
    };
    let loading = ScriptLoad::new(&script)?;
    if !added {
        let head = document.query_selector("head").map_err(js_error)?.ok_or("No head")?;
        head.append_child(&script).map_err(js_error)?;
    }
    // The SDK may already have become available while a prior caller waited.
    if present(&window).is_none() {
        loading.await?;
    }
    match present(&window) {
        Some(global) => Ok(global),
        None => {
            script.remove();
            Err("Messages didn't load".into())
        }
    }
}

#[derive(Default)]
struct LoadState {
    outcome: Option<Result<(), &'static str>>,
    waker: Option<Waker>,
}

impl LoadState {
    /// A late error/timeout must never replace the result that woke the caller.
    fn finish(state: &RefCell<Self>, outcome: Result<(), &'static str>) {
        let waker = {
            let mut state = state.borrow_mut();
            if state.outcome.is_some() {
                return;
            }
            state.outcome = Some(outcome);
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// Unlike `Closure::once_into_js`, these callbacks have an owner even when the
/// async caller is cancelled. Dropping the future detaches both listeners,
/// clears the timer and drops the closures; no forgotten Rust callback remains.
struct ScriptLoad {
    script: web_sys::Element,
    state: Rc<RefCell<LoadState>>,
    loaded: Closure<dyn FnMut()>,
    failed: Closure<dyn FnMut()>,
    timeout: Closure<dyn FnMut()>,
    timer: Option<JsValue>,
}

impl ScriptLoad {
    fn new(script: &web_sys::Element) -> Result<Self, String> {
        let waiters = waiter_count(script).checked_add(1).ok_or("Too many database bridge loads")?;
        script.set_attribute(WAITERS, &waiters.to_string()).map_err(js_error)?;
        let state = Rc::new(RefCell::new(LoadState::default()));
        let loaded_state = state.clone();
        let failed_state = state.clone();
        let timeout_state = state.clone();
        let mut loading = Self {
            script: script.clone(),
            state,
            loaded: Closure::new(move || LoadState::finish(&loaded_state, Ok(()))),
            failed: Closure::new(move || LoadState::finish(&failed_state, Err(LOAD_FAILED))),
            timeout: Closure::new(move || LoadState::finish(&timeout_state, Err(LOAD_TIMEOUT))),
            timer: None,
        };
        script.add_event_listener_with_callback("load", loading.loaded.as_ref().unchecked_ref()).map_err(js_error)?;
        script.add_event_listener_with_callback("error", loading.failed.as_ref().unchecked_ref()).map_err(js_error)?;
        loading.timer = Some(set_timeout(loading.timeout.as_ref(), LOAD_TIMEOUT_MS)?);
        Ok(loading)
    }
}

fn waiter_count(script: &web_sys::Element) -> usize {
    script.get_attribute(WAITERS).and_then(|s| s.parse().ok()).unwrap_or(0)
}

impl Future for ScriptLoad {
    type Output = Result<(), String>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.borrow_mut();
        match state.outcome {
            Some(outcome) => Poll::Ready(outcome.map_err(str::to_owned)),
            None => {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl Drop for ScriptLoad {
    fn drop(&mut self) {
        let _ = self.script.remove_event_listener_with_callback("load", self.loaded.as_ref().unchecked_ref());
        let _ = self.script.remove_event_listener_with_callback("error", self.failed.as_ref().unchecked_ref());
        if let Some(timer) = self.timer.take() {
            clear_timeout(&timer);
        }
        let remaining = waiter_count(&self.script).saturating_sub(1);
        if remaining == 0 {
            let _ = self.script.remove_attribute(WAITERS);
        } else {
            let _ = self.script.set_attribute(WAITERS, &remaining.to_string());
        }
        let outcome = self.state.borrow().outcome;
        // Cancelling the final pending waiter must not leave a tag whose load
        // event nobody can observe. Other concurrent callers keep theirs.
        if outcome.is_some_and(|result| result.is_err()) || (outcome.is_none() && remaining == 0) {
            self.script.remove();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    use super::*;

    struct Woken(AtomicUsize);
    impl Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn first_load_result_wins_and_wakes_only_once() {
        let woken = Arc::new(Woken(AtomicUsize::new(0)));
        let state = RefCell::new(LoadState { outcome: None, waker: Some(Waker::from(woken.clone())) });
        LoadState::finish(&state, Err(LOAD_FAILED));
        LoadState::finish(&state, Ok(()));
        LoadState::finish(&state, Err(LOAD_TIMEOUT));
        assert_eq!(state.borrow().outcome, Some(Err(LOAD_FAILED)));
        assert!(state.borrow().waker.is_none());
        assert_eq!(woken.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn load_before_the_first_poll_is_not_lost() {
        let state = RefCell::new(LoadState::default());
        LoadState::finish(&state, Ok(()));
        LoadState::finish(&state, Err(LOAD_TIMEOUT));
        assert_eq!(state.borrow().outcome, Some(Ok(())));
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! Code fetched the first time it's used. A function marked
//! `#[wasm_split(<part>, ...)]` is cut out of the app's module when it's
//! built (`cargo xtask`, with wasm-split) into a module of its own, which
//! the browser fetches the first time the function is called; calling it
//! again is immediate. See docs/PERFORMANCE.md#code-loaded-when-used.

use std::future::Future;

use leptos::prelude::*;
use leptos::wasm_split::SplitLoaderError;
use send_wrapper::SendWrapper;

/// A page whose code is fetched on first use: `pending` until it has come
/// (nothing at all once it has), then the page. If it can't come, the page
/// is loaded again (see [`reload_once`]), with only `pending` meanwhile,
/// and if that doesn't bring it, says so.
pub fn lazy_page<P, V, F>(pending: P, view: F) -> impl IntoView
where
    P: Fn() -> V + Send + Sync + 'static,
    V: IntoView + 'static,
    F: Future<Output = Result<AnyView, SplitLoaderError>> + 'static,
{
    lazy(pending, view, true, || {
        view! {
            <section class="error-panel" role="alert">
                <h2>"This part of Allthing couldn't load"</h2>
                <p>"Check the connection and try again."</p>
                <button type="button" class="button" on:click=move |_| { let _ = window().location().reload(); }>"Try again"</button>
            </section>
        }
        .into_any()
    })
}

/// Part of a page (a dialog's, a picker) whose code is fetched on first
/// use: `pending` until it has come (nothing at all once it has), then the
/// view. If it can't come, it says so in its place, with Reload: the page
/// isn't loaded again behind the reader's back, so nothing they're doing is
/// lost unasked. (The static copy has none: no composer, no React.)
#[cfg(not(feature = "static-host"))]
pub fn lazy_view<P, V, F>(pending: P, view: F) -> impl IntoView
where
    P: Fn() -> V + Send + Sync + 'static,
    V: IntoView + 'static,
    F: Future<Output = Result<AnyView, SplitLoaderError>> + 'static,
{
    lazy(pending, view, false, || {
        view! {
            <p class="part-unloaded" role="alert">
                "This part of Allthing couldn't load. "
                <button type="button" class="link-button" on:click=move |_| { let _ = window().location().reload(); }>"Reload"</button>
            </p>
        }
        .into_any()
    })
}

fn lazy<P, V, F>(pending: P, view: F, reload: bool, unloaded: fn() -> AnyView) -> impl IntoView
where
    P: Fn() -> V + Send + Sync + 'static,
    V: IntoView + 'static,
    F: Future<Output = Result<AnyView, SplitLoaderError>> + 'static,
{
    let came = RwSignal::new(false);
    // Built first: code already in shows at once, with no `pending` before.
    let part = Suspend::new(SendWrapper::new(async move {
        match view.await {
            Ok(view) => {
                came.set(true);
                view
            }
            // While the page loads again, `pending` stays.
            Err(_) if reload && reload_once() => ().into_any(),
            Err(_) => {
                came.set(true);
                unloaded()
            }
        }
    }));
    view! { {part} {move || (!came.get()).then(&pending)} }
}

/// The session key that says this build has already reloaded the page.
const RELOADED: &str = "allthing-reloaded";

/// Loads the page again, once per build and tab, and says whether it does.
/// A part of the app fails to come when Allthing has been deployed since
/// the page was opened: the previous version's files are gone, and the
/// page, loaded again, is the new version. Once is enough; a second failure
/// is shown instead.
fn reload_once() -> bool {
    let Some(storage) = window().session_storage().ok().flatten() else { return false };
    if storage.get_item(RELOADED).ok().flatten().as_deref() == Some(crate::COMMIT) {
        return false;
    }
    storage.set_item(RELOADED, crate::COMMIT).is_ok() && window().location().reload().is_ok()
}

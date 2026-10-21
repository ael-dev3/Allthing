// SPDX-License-Identifier: AGPL-3.0-or-later
//! Code fetched the first time it's used. A function marked
//! `#[wasm_split(<part>, ...)]` is cut out of the app's module when it's
//! built (`cargo xtask`, with wasm-split) into a module of its own, which
//! the browser fetches the first time the function is called; calling it
//! again is immediate. See docs/PERFORMANCE.md#code-loaded-when-used.

use std::future::Future;
use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::wasm_split::SplitLoaderError;
use send_wrapper::SendWrapper;
use wasm_bindgen::JsCast;

use crate::components::cast::{CastSkeletons, idle};
use crate::util::document;

/// How long after the app starts a post page's code is fetched ahead of
/// use.
const POST_AFTER: Duration = Duration::from_secs(3);

/// A page of the app's own, its code fetched on first use: as
/// [`lazy_page`], with placeholder cards meanwhile.
pub fn page<F>(view: F) -> impl IntoView
where
    F: Future<Output = Result<AnyView, SplitLoaderError>> + 'static,
{
    lazy_page(|| view! { <CastSkeletons count=2/> }, view)
}

/// Fetch the pages' code ahead of use, as it's likely wanted, so a page
/// opens as it did when its code came with the app:
/// - a post's a few seconds after the app starts, once the browser is idle
///   (opening a post is how most visits go on from a feed), and as a card
///   is pointed at or touched (`CastCard`);
/// - a profile's, a channel's, search's, the channels' and the reader's
///   notifications' as a link to one is pointed at, touched, pressed or
///   focused, anywhere on the page (the header's bell, say), and search's
///   as the search box is; the data of a post or a channel a link opens
///   comes then too (`prefetch_page_data`).
///
/// A visit that starts on one of them has `index.html` fetch its code with
/// the app. On a slow or metered connection only intent fetches anything.
pub fn preload_pages() {
    let intent = wasm_bindgen::closure::Closure::<dyn Fn(web_sys::Event)>::new(|event: web_sys::Event| {
        let Some(el) = event.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        if let Some(link) = el.closest("a[href]").ok().flatten()
            && let Some(path) = link.get_attribute("href")
        {
            preload_page(&path);
            prefetch_page_data(&event, link, &path);
        } else if el.closest("form[role=search]").ok().flatten().is_some() {
            // Search's results, as the search box is used.
            preload_page("/search");
        }
    });
    for name in ["pointerover", "pointerdown", "touchstart", "focusin"] {
        let _ = document().add_event_listener_with_callback_and_bool(name, intent.as_ref().unchecked_ref(), true);
    }
    intent.forget();
    if crate::api::constrained_network() {
        return;
    }
    set_timeout(|| idle(|| preload_page(&crate::util::href("/p/"))), POST_AFTER);
}

/// How long a mouse rests on a link before the data of the page it opens is
/// fetched: passing over a link isn't meaning to open it.
const HOVER_INTENT: Duration = Duration::from_millis(100);

/// The data of a post or a channel a link opens, ahead of the press that
/// opens it, through the prefetches' cap: at once for a touch, a press or
/// keyboard focus, and once a mouse has rested on the link for
/// `HOVER_INTENT`. (Cards fetch their post's as they're pointed at.) Not a
/// profile's: a name pointed at or tapped opens the account's card, which
/// never reads the profile's payload, and a profile opened from a feed
/// shows its header at once anyway (`api::seen_person`).
fn prefetch_page_data(event: &web_sys::Event, link: web_sys::Element, href: &str) {
    let page = href.strip_prefix(crate::util::BASE).unwrap_or(href);
    if !["/p/", "/c/"].iter().any(|prefix| page.starts_with(prefix)) {
        return;
    }
    let Some(read) = crate::nav::page_read(href) else { return };
    let pointer = js_sys::Reflect::get(event, &"pointerType".into()).ok().and_then(|t| t.as_string());
    let hovered = event.type_() == "pointerover" && pointer.as_deref() == Some("mouse");
    if !hovered {
        crate::api::prefetch(&read.api);
        return;
    }
    set_timeout(
        move || {
            if link.matches(":hover").unwrap_or(false) {
                crate::api::prefetch(&read.api);
            }
        },
        HOVER_INTENT,
    );
}

thread_local! {
    /// The pages whose code has been asked for, by `PAGES` index.
    static ASKED: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    /// Of those, the pages whose code has come.
    static CAME: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

/// Whether the page at `path` shows at once as far as its code goes: it
/// isn't a part of its own, or its code, asked for, has come. (Not asked
/// for here, it's fetched as the page opens, with its placeholder.) A link
/// pressed asks for it, so `nav::go` waits for it as for the page's data:
/// a channel's data that came before its code opened it on the placeholder
/// of a page whose code hadn't come, for a frame or more.
pub fn page_came(path: &str) -> bool {
    let Some(part) = page_part(path) else { return true };
    let bit = 1u8 << part;
    ASKED.with(std::cell::Cell::get) & bit == 0 || CAME.with(std::cell::Cell::get) & bit != 0
}

/// The page whose code a link to `path` needs, if it's one of the parts.
fn page_part(path: &str) -> Option<usize> {
    let path = path.strip_prefix(crate::util::BASE).unwrap_or(path);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    [
        path.starts_with("/p/") || path == "/p",
        path.starts_with("/u/") || path == "/u",
        path.starts_with("/c/") || path == "/c",
        path == "/search",
        path == "/channels",
        // The static copy has no notifications.
        cfg!(not(feature = "static-host")) && path == "/notifications",
    ]
    .iter()
    .position(|&is| is)
}

/// Start fetching the code of the page `path` is, once.
pub fn preload_page(path: &str) {
    let Some(part) = page_part(path) else { return };
    let bit = 1u8 << part;
    if ASKED.with(|a| a.replace(a.get() | bit)) & bit != 0 {
        return;
    }
    spawn_local(async move {
        let fetched = match part {
            0 => crate::pages::post::preload_post().await,
            1 => crate::pages::user::preload_user().await,
            2 => crate::pages::channel::preload_channel().await,
            3 => crate::pages::search::preload_search().await,
            #[cfg(not(feature = "static-host"))]
            5 => crate::pages::notifications::with_worker::preload_notifications().await,
            _ => crate::pages::channels::preload_channels().await,
        };
        // Couldn't come: the next intent asks again.
        if fetched.is_err() {
            ASKED.with(|a| a.set(a.get() & !bit));
        } else {
            CAME.with(|c| c.set(c.get() | bit));
        }
    });
}

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

/// Something a card does without when it must (its emoji reactions), its
/// code fetched on first use: `pending` until it has come, then the view;
/// if it can't come, nothing, as on a card that has none.
#[cfg(not(feature = "static-host"))]
pub fn lazy_extra<P, V, F>(pending: P, view: F) -> impl IntoView
where
    P: Fn() -> V + Send + Sync + 'static,
    V: IntoView + 'static,
    F: Future<Output = Result<AnyView, SplitLoaderError>> + 'static,
{
    lazy(pending, view, false, || ().into_any())
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

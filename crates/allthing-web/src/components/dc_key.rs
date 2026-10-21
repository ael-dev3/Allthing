// SPDX-License-Identifier: AGPL-3.0-or-later
//! The reader's Farcaster API key for Direct Casts, as this browser keeps
//! it (`keys::dc_key`), and the account menu's Messages and Forget key.
//! Signing out forgets the key, as it does the app key, and the site's
//! other tabs hear when it changes. The Messages page itself is a part of
//! its own (`pages::messages`). Not in the static copy, which has no
//! sign-in.

use std::cell::RefCell;

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::components::signin::use_session;
use crate::keys;
use crate::util::messages_href;

/// The other tabs of the site hear on this when the key is kept or
/// forgotten.
const TABS: &str = "allthing-dc";

/// What follows the key kept for whoever is signed in.
#[derive(Clone, Copy)]
pub struct DcKey {
    /// Moves on whenever that key may have changed: kept or forgotten
    /// here, in another tab, or by signing out.
    changed: RwSignal<u32>,
}

thread_local! {
    static TABS_CHANNEL: RefCell<Option<web_sys::BroadcastChannel>> = const { RefCell::new(None) };
}

impl DcKey {
    /// The app's: it follows who is signed in.
    pub fn new() -> DcKey {
        let session = use_session();
        let dc = DcKey { changed: RwSignal::new(0) };
        Effect::new(move |before: Option<Option<u64>>| {
            let fid = session.user.with(|u| u.as_ref().map(|u| u.fid));
            // Signing out forgets the key: whoever uses this browser next
            // can't read the reader's Direct Casts.
            if let Some(Some(gone)) = before
                && fid != Some(gone)
            {
                spawn_local(async move {
                    let _ = keys::forget_dc_key(gone).await;
                    dc.bump();
                });
            }
            fid
        });
        dc.follow_tabs();
        dc
    }

    /// A number that changes with the key: read it to follow it.
    pub fn version(self) -> u32 {
        self.changed.get()
    }

    fn bump(self) {
        self.changed.update(|n| *n = n.wrapping_add(1));
    }

    /// The key was kept or forgotten here: follow it, and tell the other
    /// tabs.
    pub fn changed(self) {
        self.bump();
        TABS_CHANNEL.with(|c| {
            if let Some(channel) = c.borrow().as_ref() {
                let _ = channel.post_message(&JsValue::from_str("changed"));
            }
        });
    }

    /// Forget the key kept for `fid`.
    pub fn forget(self, fid: u64) {
        spawn_local(async move {
            let _ = keys::forget_dc_key(fid).await;
            self.changed();
        });
    }

    fn follow_tabs(self) {
        let Ok(channel) = web_sys::BroadcastChannel::new(TABS) else { return };
        let heard = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |_: web_sys::MessageEvent| self.bump());
        channel.set_onmessage(Some(heard.as_ref().unchecked_ref()));
        heard.forget();
        TABS_CHANNEL.with(|c| *c.borrow_mut() = Some(channel));
    }
}

impl Default for DcKey {
    fn default() -> Self {
        DcKey::new()
    }
}

pub fn use_dc_key() -> DcKey {
    expect_context::<DcKey>()
}

/// The account menu's link to the reader's Direct Casts. Pointing at it,
/// or focusing it, fetches the page's code.
#[component]
pub fn MessagesMenuItem() -> impl IntoView {
    use crate::pages::messages::warm;
    view! {
        <a
            class="account-link"
            href=messages_href()
            on:pointerenter=move |_| warm()
            on:focus=move |_| warm()
            on:touchstart=move |_| warm()
        >
            "Messages"
        </a>
    }
}

/// The account menu's Forget key, while this browser keeps one for the
/// reader.
#[component]
pub fn DcKeyMenuItem() -> impl IntoView {
    let session = use_session();
    let dc = use_dc_key();
    // Whose key this browser keeps, read as the menu opens.
    let kept = RwSignal::new(None::<u64>);
    Effect::new(move |_| {
        let fid = session.user.with(|u| u.as_ref().map(|u| u.fid));
        let _ = dc.version();
        spawn_local(async move {
            let found = match fid {
                Some(fid) => keys::dc_key(fid).await.ok().flatten().map(|_| fid),
                None => None,
            };
            let _ = kept.try_set(found);
        });
    });
    move || {
        kept.get().map(|fid| {
            view! {
                <button type="button" class="account-link forget-dc-key" on:click=move |_| dc.forget(fid)>
                    "Forget Direct Cast key"
                </button>
            }
        })
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! Handing a write to the Farcaster app. Where Allthing doesn't write for
//! the reader (signed out, or where the Worker endorses no keys: on
//! allthing.social, until the `ALLTHING_CUSTODY_KEY` secret is set), a like,
//! a recast, a follow, a delete or a Pepe reaction opens this dialog. It
//! sends the reader to the cast, the profile or a reply ready to post, in
//! the Farcaster app (`allthing_core::compose::cast_url` and the like), to
//! do it there as themselves. Where a pointer hovers (a computer), the link
//! shows as a QR code for the reader's phone, drawn by the Worker
//! (`/api/v1/auth/qr`, which draws only Farcaster's links), and opens on
//! farcaster.xyz; on a phone it opens the app. A draft from the composer
//! goes the same way on a computer; on a phone, its "Post in Farcaster"
//! opens the app at once (`components::compose`). Not in the static copy.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use allthing_core::siwf::{self, QrCode, QrRequest};
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::HtmlElement;

use crate::api;
use crate::util::{can_hover, document, trap_tab};

const OUT: &str = "M9 3h4v4M13 3L7.5 8.5M11.5 9.5v3h-8v-8h3";

thread_local! {
    /// Codes drawn this visit, by link: the same hand-off again asks for none.
    static CODES: RefCell<HashMap<String, QrCode>> = RefCell::new(HashMap::new());
    static RETURN_FOCUS: RefCell<Option<HtmlElement>> = const { RefCell::new(None) };
}

/// A write to hand off: the dialog's title, a line on what to do in the
/// app where the title doesn't say it all, and the link that opens it there.
#[derive(Clone)]
pub struct Ask {
    pub title: String,
    pub line: Option<String>,
    pub url: String,
    /// A draft from the composer: cleared, and the composer closed, once it
    /// has gone to the app.
    pub done: Option<Callback<()>>,
    /// Closed without going to the app: what the write shows here is taken
    /// back (`Writer::hand_off`).
    pub dismissed: Option<Dismissed>,
}

/// Run when a hand-off closes without going to the app. Not a `Callback`:
/// what opened it may be gone by then.
pub type Dismissed = std::sync::Arc<dyn Fn() + Send + Sync>;

impl Ask {
    fn new(title: impl Into<String>, line: Option<String>, url: String) -> Ask {
        Ask { title: title.into(), line, url, done: None, dismissed: None }
    }

    /// A like, or, for a cast the reader likes, taking it back.
    pub fn like(url: String, liked: bool) -> Ask {
        Ask::new(if liked { "Unlike in Farcaster" } else { "Like in Farcaster" }, None, url)
    }

    /// A recast, or, for a cast the reader recasted, undoing it.
    pub fn recast(url: String, recasted: bool) -> Ask {
        Ask::new(if recasted { "Undo recast in Farcaster" } else { "Recast in Farcaster" }, None, url)
    }

    pub fn follow(url: String, username: &str, following: bool) -> Ask {
        let verb = if following { "Unfollow" } else { "Follow" };
        Ask::new(format!("{verb} @{username} in Farcaster"), None, url)
    }

    pub fn delete(url: String, noun: &str) -> Ask {
        Ask::new("Delete in Farcaster", Some(format!("Delete your {noun} from its menu there.")), url)
    }

    pub fn react(url: String, code: &str) -> Ask {
        Ask::new("React in Farcaster", Some(format!("A reply with :{code}:, ready to post.")), url)
    }

    pub fn post(url: String, noun: &str, done: Callback<()>) -> Ask {
        Ask { done: Some(done), ..Ask::new("Post in Farcaster", Some(format!("Your {noun}, ready to post.")), url) }
    }
}

/// The hand-off the dialog shows, if any.
#[derive(Clone, Copy)]
pub struct HandOff {
    ask: RwSignal<Option<Ask>>,
}

impl HandOff {
    pub fn new() -> HandOff {
        HandOff { ask: RwSignal::new(None) }
    }

    pub fn open(self, ask: Ask) {
        RETURN_FOCUS.with(|r| *r.borrow_mut() = document().active_element().and_then(|el| el.dyn_into().ok()));
        self.ask.set(Some(ask));
    }

    /// Close without going to the app: the reader changed their mind.
    pub fn dismiss(self) {
        if let Some(dismissed) = self.ask.with_untracked(|ask| ask.as_ref().and_then(|ask| ask.dismissed.clone())) {
            dismissed();
        }
        self.close();
    }

    /// Close, if it still shows the hand-off to `url`: done in the app.
    pub fn close_if(self, url: &str) {
        if self.ask.with_untracked(|ask| ask.as_ref().is_some_and(|ask| ask.url == url)) {
            self.close();
        }
    }

    /// Close, and return focus to what opened it.
    pub fn close(self) {
        self.ask.set(None);
        if let Some(element) = RETURN_FOCUS.with(|r| r.borrow_mut().take()).filter(|el| el.is_connected()) {
            let _ = element.focus();
        }
    }
}

impl Default for HandOff {
    fn default() -> Self {
        HandOff::new()
    }
}

pub fn use_hand_off() -> HandOff {
    expect_context::<HandOff>()
}

/// `url` as a QR code, drawn once a visit.
async fn code_for(url: String) -> Option<QrCode> {
    if let Some(code) = CODES.with(|c| c.borrow().get(&url).cloned()) {
        return Some(code);
    }
    let code: QrCode = api::post(&siwf::qr_path(), &QrRequest { url: url.clone() }).await.ok()?;
    CODES.with(|c| c.borrow_mut().insert(url, code.clone()));
    Some(code)
}

#[component]
pub fn HandOffDialog() -> impl IntoView {
    let hand_off = use_hand_off();
    let dialog = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let open = hand_off.ask.with(Option::is_some);
        if let Some(root) = document().document_element() {
            let _ = root.class_list().toggle_with_force("handoff-open", open);
        }
    });
    let keys = window_event_listener(ev::keydown, move |ev| {
        if hand_off.ask.with_untracked(Option::is_none) {
            return;
        }
        match ev.key().as_str() {
            "Escape" => {
                ev.prevent_default();
                hand_off.dismiss();
            }
            "Tab" => {
                if let Some(root) = dialog.get_untracked() {
                    trap_tab(&root, &ev, "button, a[href]");
                }
            }
            _ => {}
        }
    });
    on_cleanup(move || keys.remove());
    move || {
        hand_off.ask.get().map(|ask| {
            // A phone opens the link in the app; a computer shows it as a
            // code for the reader's phone, too.
            let computer = can_hover();
            let url = ask.url.clone();
            let code = computer.then(|| LocalResource::new(move || code_for(url.clone())));
            let open_link = NodeRef::<leptos::html::A>::new();
            Effect::new(move |_| {
                if let Some(link) = open_link.get() {
                    let _ = link.focus();
                }
            });
            let done = ask.done;
            let finish = move || {
                if let Some(done) = done {
                    done.run(());
                }
                hand_off.close();
            };
            view! {
                <div class="modal-backdrop" on:click=move |ev: ev::MouseEvent| {
                    if ev.target() == ev.current_target() {
                        hand_off.dismiss();
                    }
                }>
                    <div class="modal signin-modal handoff-modal" role="dialog" aria-modal="true" aria-labelledby="handoff-title" node_ref=dialog>
                        <header class="modal-head">
                            <h2 id="handoff-title">{ask.title.clone()}</h2>
                            <button type="button" class="modal-close" aria-label="Close" on:click=move |_| hand_off.dismiss()>"×"</button>
                        </header>
                        <div class="modal-body signin-body">
                            {code.map(|code| view! {
                                <p class="signin-intro">"Scan with your phone."</p>
                                <div class="qr-frame">
                                    {move || match code.get() {
                                        None => view! { <span class="qr-loading" aria-hidden="true"></span> }.into_any(),
                                        Some(Some(code)) => view! {
                                            <svg class="qr" viewBox=format!("0 0 {0} {0}", code.size) role="img" aria-label="QR code that opens it in the Farcaster app" shape-rendering="crispEdges">
                                                <path d=code.path/>
                                            </svg>
                                        }
                                        .into_any(),
                                        Some(None) => view! { <span class="qr-none muted small">"No code for this one. Open it below."</span> }.into_any(),
                                    }}
                                </div>
                            })}
                            {ask.line.clone().map(|line| view! { <p class="handoff-line">{line}</p> })}
                            <a
                                class="button handoff-open"
                                href=ask.url.clone()
                                target="_blank"
                                rel="noopener noreferrer"
                                node_ref=open_link
                                // After the link has opened.
                                on:click=move |_| set_timeout(finish, Duration::ZERO)
                            >
                                {if computer { "Open on farcaster.xyz" } else { "Open in Farcaster" }}
                                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=OUT/></svg>
                            </a>
                            {done.map(|_| view! { <button type="button" class="handoff-done" on:click=move |_| finish()>"Done"</button> })}
                        </div>
                    </div>
                </div>
            }
        })
    }
}

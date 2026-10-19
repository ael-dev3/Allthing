// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sign In With Farcaster, by QR code: the header's "Sign in" button, or the
//! signed-in account's picture and its menu, and the dialog that shows the
//! code. The Farcaster app on the person's phone approves the sign-in; the
//! dialog polls Farcaster's relay until it has, and the Worker checks what
//! the app signed and sets the session cookie (`allthing_core::siwf`).
//! Not in the static copy, which has no Worker.

use std::cell::{Cell, RefCell};

use allthing_core::model::UserSummary;
use allthing_core::siwf::{self, Proof, QrCode, QrRequest, RELAY, SignedIn, Started, USER_COOKIE};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlElement;

use crate::api;
use crate::components::chips::Avatar;
use crate::components::popover::Popover;
use crate::util::{document, user_href};

/// How often the dialog asks the relay whether the app has answered, and
/// how long a code lasts.
const POLL_MS: i32 = 1500;
const CODE_MS: f64 = 300_000.0;
const EXPIRED: &str = "This code has expired. Get a new one to sign in.";
const RELAY_DOWN: &str = "Couldn't reach Farcaster's sign-in relay. Check your connection and try again.";

/// Where a sign-in stands.
#[derive(Clone, PartialEq)]
enum Step {
    /// Asking the Worker for a nonce, the relay for a channel, the Worker
    /// for the code.
    Starting,
    /// The code is up; the Farcaster app hasn't answered yet.
    Waiting {
        code: QrCode,
    },
    /// The app approved; the Worker is checking what it signed.
    Checking,
    Failed(String),
}

/// Who is signed in, as the Worker's user cookie says, and the sign-in
/// dialog's state. Held by the app, so a sign-in finishing after its dialog
/// closed has somewhere to land.
#[derive(Clone, Copy)]
pub struct Session {
    pub user: RwSignal<Option<UserSummary>>,
    open: RwSignal<bool>,
    step: RwSignal<Step>,
}

thread_local! {
    /// The sign-in under way: starting another, or closing the dialog,
    /// moves this on, and the older one stops.
    static RUN: Cell<u64> = const { Cell::new(0) };
    static RETURN_FOCUS: RefCell<Option<HtmlElement>> = const { RefCell::new(None) };
}

impl Session {
    pub fn new() -> Session {
        Session { user: RwSignal::new(cookie_user()), open: RwSignal::new(false), step: RwSignal::new(Step::Starting) }
    }

    fn open(self) {
        RETURN_FOCUS.with(|r| *r.borrow_mut() = document().active_element().and_then(|el| el.dyn_into().ok()));
        self.open.set(true);
        begin(self);
    }

    /// Close the dialog, stopping its sign-in, and return focus to where it
    /// was.
    fn close(self) {
        self.dismiss();
        if let Some(element) = RETURN_FOCUS.with(|r| r.borrow_mut().take()).filter(|el| el.is_connected()) {
            let _ = element.focus();
        }
    }

    /// Signed in: the account's button replaces the one that opened the
    /// dialog, and takes the focus.
    fn signed_in(self, user: UserSummary) {
        self.user.set(Some(user));
        self.dismiss();
        RETURN_FOCUS.with(|r| r.borrow_mut().take());
        focus_soon(".account-button");
    }

    fn dismiss(self) {
        RUN.with(|run| run.set(run.get() + 1));
        self.open.set(false);
    }

    fn sign_out(self) {
        spawn_local(async {
            let _ = api::post::<_, IgnoredAny>(&siwf::sign_out_path(), &Nothing {}).await;
        });
        // Signed out here even if the request doesn't get through.
        let _ = js_sys::Reflect::set(
            &document(),
            &JsValue::from_str("cookie"),
            &JsValue::from_str(&format!("{USER_COOKIE}=; Path=/; Max-Age=0; Secure; SameSite=Lax")),
        );
        self.user.set(None);
        // The menu's button goes with the menu: focus its replacement.
        focus_soon(".sign-in-button");
    }
}

/// Focus `selector`'s element once the page has drawn it.
fn focus_soon(selector: &'static str) {
    request_animation_frame(move || {
        if let Some(element) = document().query_selector(selector).ok().flatten() {
            let _ = element.unchecked_into::<HtmlElement>().focus();
        }
    });
}

pub fn use_session() -> Session {
    expect_context::<Session>()
}

/// Who the user cookie names, if the Worker signed someone in.
fn cookie_user() -> Option<UserSummary> {
    let cookies = js_sys::Reflect::get(&document(), &JsValue::from_str("cookie")).ok()?.as_string()?;
    siwf::user_from_cookie(siwf::cookie(&cookies, USER_COOKIE)?)
}

fn current(run: u64) -> bool {
    RUN.with(|r| r.get() == run)
}

/// Start a sign-in with a fresh code.
fn begin(session: Session) {
    let run = RUN.with(|r| {
        r.set(r.get() + 1);
        r.get()
    });
    session.step.set(Step::Starting);
    spawn_local(async move {
        match sign_in(run, session.step).await {
            Ok(user) if current(run) => session.signed_in(user),
            // The dialog closed while the Worker was checking, which it then
            // accepted: the session cookie is set, so the header says so.
            Ok(user) => session.user.set(Some(user)),
            Err(message) if current(run) => session.step.set(Step::Failed(message)),
            Err(_) => {}
        }
    });
}

/// An empty JSON body (`{}`).
#[derive(Serialize)]
struct Nothing {}

/// What the relay opens a channel with, as AuthKit sends it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelRequest<'a> {
    siwe_uri: &'a str,
    domain: &'a str,
    nonce: &'a str,
    /// The Worker accepts an auth address as the signer, as well as the
    /// custody address.
    accept_auth_address: bool,
}

/// The relay's new channel.
#[derive(Deserialize)]
struct Channel {
    #[serde(rename = "channelToken")]
    token: String,
    url: String,
}

/// The relay's word on a channel.
#[derive(Deserialize)]
struct Status {
    state: String,
    message: Option<String>,
    signature: Option<String>,
}

async fn sign_in(run: u64, step: RwSignal<Step>) -> Result<UserSummary, String> {
    let failed = |failure: api::Failure| failure.message();
    let started: Started = api::post(&siwf::start_path(), &Nothing {}).await.map_err(failed)?;
    let request = ChannelRequest {
        siwe_uri: &started.uri,
        domain: &started.domain,
        nonce: &started.nonce,
        accept_auth_address: true,
    };
    let request = serde_json::to_string(&request).map_err(|_| RELAY_DOWN.to_owned())?;
    let (status, body) = relay("POST", "channel", None, Some(request)).await?;
    let channel = serde_json::from_str::<Channel>(&body)
        .ok()
        .filter(|c| (200..300).contains(&status) && siwf::is_app_link(&c.url))
        .ok_or_else(|| RELAY_DOWN.to_owned())?;
    let code: QrCode = api::post(&siwf::qr_path(), &QrRequest { url: channel.url.clone() }).await.map_err(failed)?;
    if !current(run) {
        return Err(String::new());
    }
    step.set(Step::Waiting { code });
    let proof = approved(run, &channel.token).await?;
    if !current(run) {
        return Err(String::new());
    }
    step.set(Step::Checking);
    let signed: SignedIn = api::post(&siwf::verify_path(), &proof).await.map_err(failed)?;
    Ok(signed.user)
}

/// What the app signed, once the person approves; polled until the code
/// expires. A poll that fails is tried again at the next.
async fn approved(run: u64, token: &str) -> Result<Proof, String> {
    let until = js_sys::Date::now() + CODE_MS;
    while js_sys::Date::now() < until {
        crate::video::sleep(POLL_MS).await;
        if !current(run) {
            return Err(String::new());
        }
        let Ok((status, body)) = relay("GET", "channel/status", Some(token), None).await else { continue };
        if matches!(status, 401 | 404 | 410) {
            return Err(EXPIRED.into());
        }
        if let Ok(Status { state, message: Some(message), signature: Some(signature) }) = serde_json::from_str(&body)
            && state == "completed"
        {
            return Ok(Proof { message, signature });
        }
    }
    Err(EXPIRED.into())
}

/// A request to Farcaster's relay: its status and body.
async fn relay(method: &str, path: &str, token: Option<&str>, body: Option<String>) -> Result<(u16, String), String> {
    let down = |_| RELAY_DOWN.to_owned();
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    let headers = web_sys::Headers::new().map_err(down)?;
    if let Some(token) = token {
        headers.set("authorization", &format!("Bearer {token}")).map_err(down)?;
    }
    if let Some(body) = body {
        headers.set("content-type", "application/json").map_err(down)?;
        init.set_body(&JsValue::from_str(&body));
    }
    init.set_headers(&headers);
    let window = web_sys::window().ok_or(RELAY_DOWN)?;
    let response = JsFuture::from(window.fetch_with_str_and_init(&format!("{RELAY}/v1/{path}"), &init)).await;
    let response: web_sys::Response = response.map_err(down)?.dyn_into().map_err(down)?;
    let text = JsFuture::from(response.text().map_err(down)?).await.map_err(down)?.as_string().unwrap_or_default();
    Ok((response.status(), text))
}

const PERSON: &str = "M3 13.5c.6-2.6 2.6-4 5-4s4.4 1.4 5 4";

/// The header's "Sign in" button, or the signed-in account's picture with
/// its menu: who it is, their profile, and signing out.
#[component]
pub fn Account() -> impl IntoView {
    let session = use_session();
    move || {
        match session.user.get() {
        None => view! {
            <button type="button" class="sign-in-button" aria-label="Sign in with Farcaster" on:click=move |_| session.open()>
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="5.5" r="2.6"/><path d=PERSON/></svg>
                <span class="sign-in-label">"Sign in"</span>
            </button>
        }
        .into_any(),
        Some(user) => {
            let pfp = user.pfp_url.clone();
            let label = format!("Signed in as @{}: account menu", user.username);
            let profile = (!user.username.starts_with("fid:")).then(|| user_href(&user.username));
            let (name, username) = (user.name().to_owned(), user.username.clone());
            view! {
                <Popover
                    label="Your account"
                    trigger_label=label
                    trigger_class="account-button"
                    class="account"
                    panel_class="account-menu"
                    trigger=move || view! { <Avatar url=pfp.clone() size=32/> }
                    content=move || view! {
                        <p class="account-who">
                            <strong>{name.clone()}</strong>
                            <span class="muted">"@"{username.clone()}</span>
                        </p>
                        {profile.clone().map(|href| view! { <a class="account-link" href=href>"Your profile"</a> })}
                        <button type="button" class="account-link sign-out" on:click=move |_| session.sign_out()>"Sign out"</button>
                    }
                />
            }
            .into_any()
        }
    }
    }
}

/// The sign-in dialog: the QR code to scan with the phone's camera, and how
/// it's going. A QR code only: there is no link to the Farcaster app.
#[component]
pub fn SignInDialog() -> impl IntoView {
    let session = use_session();
    let location = use_location();
    let dialog = NodeRef::<leptos::html::Div>::new();
    // A history navigation must not leave the dialog over another page.
    Effect::new(move |_| {
        let _ = (location.pathname.get(), location.search.get());
        if session.open.get_untracked() {
            session.close();
        }
    });
    Effect::new(move |_| {
        let open = session.open.get();
        if let Some(root) = document().document_element() {
            let _ = root.class_list().toggle_with_force("signin-open", open);
        }
    });
    let keys = window_event_listener(ev::keydown, move |ev| {
        if !session.open.get_untracked() {
            return;
        }
        match ev.key().as_str() {
            "Escape" => {
                ev.prevent_default();
                session.close();
            }
            // Focus stays in the dialog.
            "Tab" => {
                let Some(root) = dialog.get_untracked() else { return };
                let Ok(nodes) = root.query_selector_all("button, a[href]") else { return };
                let first = nodes.get(0).and_then(|n| n.dyn_into::<HtmlElement>().ok());
                let last = nodes.get(nodes.length().saturating_sub(1)).and_then(|n| n.dyn_into::<HtmlElement>().ok());
                let active = document().active_element();
                let at = |el: &Option<HtmlElement>| {
                    el.as_ref().map(|e| e.unchecked_ref::<web_sys::Element>().clone()) == active
                };
                if ev.shift_key() && at(&first) {
                    ev.prevent_default();
                    if let Some(last) = last {
                        let _ = last.focus();
                    }
                } else if !ev.shift_key() && at(&last) {
                    ev.prevent_default();
                    if let Some(first) = first {
                        let _ = first.focus();
                    }
                }
            }
            _ => {}
        }
    });
    on_cleanup(move || keys.remove());
    let status = move || match session.step.get() {
        Step::Starting => "Getting a sign-in code…".to_owned(),
        Step::Waiting { .. } => "Waiting for you to approve in Farcaster…".to_owned(),
        Step::Checking => "Approved. Signing you in…".to_owned(),
        Step::Failed(message) => message,
    };
    move || {
        session.open.get().then(|| {
            let close_button = NodeRef::<leptos::html::Button>::new();
            Effect::new(move |_| {
                if let Some(button) = close_button.get() {
                    let _ = button.focus();
                }
            });
            view! {
                <div class="modal-backdrop" on:click=move |ev: ev::MouseEvent| {
                    if ev.target() == ev.current_target() {
                        session.close();
                    }
                }>
                    <div class="modal signin-modal" role="dialog" aria-modal="true" aria-labelledby="signin-title" node_ref=dialog>
                        <header class="modal-head">
                            <h2 id="signin-title">"Sign in with Farcaster"</h2>
                            <button type="button" class="modal-close" aria-label="Close" node_ref=close_button on:click=move |_| session.close()>"×"</button>
                        </header>
                        <div class="modal-body signin-body">
                            <p class="signin-intro">"Scan the code with your phone's camera, then approve in the Farcaster app."</p>
                            <div class="qr-frame" class:failed=move || matches!(session.step.get(), Step::Failed(_))>
                                {move || match session.step.get() {
                                    Step::Waiting { code } => view! {
                                        <svg class="qr" viewBox=format!("0 0 {0} {0}", code.size) role="img" aria-label="Sign-in QR code" shape-rendering="crispEdges">
                                            <path d=code.path/>
                                        </svg>
                                    }
                                    .into_any(),
                                    Step::Checking => view! { <span class="qr-check" aria-hidden="true">"✓"</span> }.into_any(),
                                    Step::Failed(_) => view! {
                                        <button type="button" class="button" on:click=move |_| begin(session)>"Get a new code"</button>
                                    }
                                    .into_any(),
                                    Step::Starting => view! { <span class="qr-loading" aria-hidden="true"></span> }.into_any(),
                                }}
                            </div>
                            <p class="signin-status" class:error=move || matches!(session.step.get(), Step::Failed(_)) role="status">
                                {status}
                            </p>
                            <p class="signin-note muted small">"Signing in only tells Allthing who you are. It can't post or change anything for you."</p>
                        </div>
                    </div>
                </div>
            }
        })
    }
}

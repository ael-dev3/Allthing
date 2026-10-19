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
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlElement;

use crate::api;
use crate::components::chips::Avatar;
use crate::components::popover::Popover;
use crate::util::{document, trap_tab, user_href};

/// How often the dialog asks the relay whether the app has answered, and
/// how long a code lasts.
const POLL_MS: i32 = 1500;
const CODE_MS: f64 = 300_000.0;
/// A code that runs out while the dialog is in view is replaced this many
/// times, before the dialog asks.
const RENEWALS: u32 = 2;
/// When the Worker can't check an approval just now (Optimism can't be
/// read), it is checked again this many times, after 1, 2 and 4 seconds.
const CHECK_RETRIES: u32 = 3;
const CHECK_WAIT_MS: i32 = 1000;
const EXPIRED: &str = "This code has expired. Get a new one to sign in.";
const RELAY_DOWN: &str = "Couldn't reach Farcaster's sign-in relay. Check your connection and try again.";
const UNCONFIRMED: &str = "You approved, but Allthing couldn't confirm it on Optimism just now.";
/// The other tabs of the site hear on this when someone signs in or out.
const TABS: &str = "allthing-session";

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
    /// The app approved, as `who`; the Worker is checking what it signed.
    Checking {
        who: Option<Approver>,
    },
    /// Approved, but the Worker couldn't check it just now: it is checked
    /// again on request, with the same approval, while its nonce lasts.
    Unconfirmed {
        proof: Proof,
        who: Option<Approver>,
    },
    Failed(String),
}

/// Who approved in the Farcaster app, as the relay says.
#[derive(Clone, PartialEq)]
struct Approver {
    username: String,
    pfp_url: Option<String>,
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
    /// Codes renewed since the dialog opened, or since the reader asked for
    /// a new one.
    static RENEWED: Cell<u32> = const { Cell::new(0) };
    /// The page came back into view since the last poll: poll at once.
    static WOKEN: Cell<bool> = const { Cell::new(false) };
    static TABS_CHANNEL: RefCell<Option<web_sys::BroadcastChannel>> = const { RefCell::new(None) };
}

impl Session {
    pub fn new() -> Session {
        let session = Session {
            user: RwSignal::new(cookie_user()),
            open: RwSignal::new(false),
            step: RwSignal::new(Step::Starting),
        };
        session.follow_tabs();
        session
    }

    /// When another tab signs in or out, this one follows, from the cookie
    /// they share; a sign-in there closes this tab's dialog.
    fn follow_tabs(self) {
        let Ok(channel) = web_sys::BroadcastChannel::new(TABS) else { return };
        let heard = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |_: web_sys::MessageEvent| {
            let user = cookie_user();
            if user.is_some() && self.open.get_untracked() {
                self.dismiss();
            }
            self.user.set(user);
        });
        channel.set_onmessage(Some(heard.as_ref().unchecked_ref()));
        heard.forget();
        TABS_CHANNEL.with(|c| *c.borrow_mut() = Some(channel));
    }

    fn open(self) {
        RETURN_FOCUS.with(|r| *r.borrow_mut() = document().active_element().and_then(|el| el.dyn_into().ok()));
        self.open.set(true);
        fresh(self);
    }

    /// Open the sign-in dialog, for something only a signed-in reader can
    /// do (a like, say), which follows once they have.
    pub fn sign_in(self) {
        if !self.open.get_untracked() {
            self.open();
        }
    }

    /// Whether the sign-in dialog is open.
    pub fn dialog_open(self) -> bool {
        self.open.get()
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
        tell_tabs();
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
        tell_tabs();
        // The menu's button goes with the menu: focus its replacement.
        focus_soon(".sign-in-button");
    }
}

/// Tell the site's other tabs that someone signed in or out.
fn tell_tabs() {
    TABS_CHANNEL.with(|c| {
        if let Some(channel) = c.borrow().as_ref() {
            let _ = channel.post_message(&JsValue::from_str("changed"));
        }
    });
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

/// The next sign-in run: any older one stops.
fn next_run() -> u64 {
    RUN.with(|r| {
        r.set(r.get() + 1);
        r.get()
    })
}

/// A new code, asked for by the reader (or the dialog opening): it may be
/// renewed again when it runs out.
fn fresh(session: Session) {
    RENEWED.with(|r| r.set(0));
    begin(session);
}

/// Start a sign-in with a fresh code.
fn begin(session: Session) {
    let run = next_run();
    session.step.set(Step::Starting);
    spawn_local(async move {
        let outcome = sign_in(run, session.step).await;
        settle(session, run, outcome);
    });
}

/// Check an approval the Worker couldn't check before, with no new code.
fn recheck(session: Session, proof: Proof, who: Option<Approver>) {
    let run = next_run();
    spawn_local(async move {
        let outcome = check(run, session.step, proof, who).await;
        settle(session, run, outcome);
    });
}

/// Where a sign-in ended. An empty message is a run that stopped, or one
/// that has set its own step.
fn settle(session: Session, run: u64, outcome: Result<UserSummary, String>) {
    match outcome {
        Ok(user) if current(run) => session.signed_in(user),
        // The dialog closed while the Worker was checking, which it then
        // accepted: the session cookie is set, so the header says so.
        Ok(user) => {
            session.user.set(Some(user));
            tell_tabs();
        }
        // A code that ran out while the dialog was in view is replaced.
        Err(message)
            if current(run) && message == EXPIRED && !document().hidden() && RENEWED.with(Cell::get) < RENEWALS =>
        {
            RENEWED.with(|r| r.set(r.get() + 1));
            begin(session);
        }
        Err(message) if current(run) && !message.is_empty() => session.step.set(Step::Failed(message)),
        Err(_) => {}
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    expiration_time: Option<&'a str>,
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

/// The relay's word on a channel, with who approved once the app has.
#[derive(Deserialize)]
struct Status {
    state: String,
    message: Option<String>,
    signature: Option<String>,
    username: Option<String>,
    #[serde(rename = "pfpUrl")]
    pfp_url: Option<String>,
}

async fn sign_in(run: u64, step: RwSignal<Step>) -> Result<UserSummary, String> {
    let failed = |failure: api::Failure| failure.message();
    let started: Started = api::post(&siwf::start_path(), &Nothing {}).await.map_err(failed)?;
    let request = ChannelRequest {
        siwe_uri: &started.uri,
        domain: &started.domain,
        nonce: &started.nonce,
        // The signed message lapses with the nonce.
        expiration_time: started.expiration_time.as_deref(),
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
    let (proof, who) = approved(run, &channel.token).await?;
    if !current(run) {
        return Err(String::new());
    }
    check(run, step, proof, who).await
}

/// The Worker's check of what the app signed. When it can't check just now
/// (Optimism can't be read) or can't be reached, the same approval is
/// checked again after 1, 2 and 4 seconds; then the dialog offers to try
/// again, still with no new code to scan.
async fn check(run: u64, step: RwSignal<Step>, proof: Proof, who: Option<Approver>) -> Result<UserSummary, String> {
    step.set(Step::Checking { who: who.clone() });
    let mut wait = CHECK_WAIT_MS;
    for tried in 0..=CHECK_RETRIES {
        match api::post::<_, SignedIn>(&siwf::verify_path(), &proof).await {
            Ok(signed) => return Ok(signed.user),
            Err(failure) if failure.is_transient() && tried < CHECK_RETRIES => {
                crate::video::sleep(wait).await;
                wait *= 2;
            }
            Err(failure) if failure.is_transient() => {}
            Err(failure) => return Err(failure.message()),
        }
        if !current(run) {
            return Err(String::new());
        }
    }
    step.set(Step::Unconfirmed { proof, who });
    Err(String::new())
}

/// What the app signed, and who approved, once the person approves; polled
/// until the code expires. A poll that fails is tried again at the next.
/// A hidden page doesn't ask; it asks as soon as it is back in view.
async fn approved(run: u64, token: &str) -> Result<(Proof, Option<Approver>), String> {
    let until = js_sys::Date::now() + CODE_MS;
    while js_sys::Date::now() < until {
        pause(POLL_MS).await;
        if !current(run) {
            return Err(String::new());
        }
        if document().hidden() {
            continue;
        }
        let Ok((status, body)) = relay("GET", "channel/status", Some(token), None).await else { continue };
        if matches!(status, 401 | 404 | 410) {
            return Err(EXPIRED.into());
        }
        if let Ok(Status { state, message: Some(message), signature: Some(signature), username, pfp_url }) =
            serde_json::from_str(&body)
            && state == "completed"
        {
            let who = username.filter(|u| !u.is_empty()).map(|username| Approver { username, pfp_url });
            return Ok((Proof { message, signature }, who));
        }
    }
    Err(EXPIRED.into())
}

/// Wait `ms`, or less if the page comes back into view meanwhile.
async fn pause(ms: i32) {
    let mut waited = 0;
    while waited < ms {
        if WOKEN.with(|w| w.replace(false)) {
            return;
        }
        crate::video::sleep(250).await;
        waited += 250;
    }
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

/// The header's "Sign in" button, or the signed-in account's picture: a
/// link to their profile, whose menu (on hover, on keyboard focus, or on a
/// tap on a touch screen) says who it is and signs out.
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
            let label = format!("Signed in as @{}: your profile", user.username);
            let profile = user_href(&user.username);
            let (name, username) = (user.name().to_owned(), user.username.clone());
            let link = profile.clone();
            view! {
                <Popover
                    label="Your account"
                    hover=true
                    links=true
                    class="account"
                    panel_class="account-menu"
                    trigger=move || view! {
                        <a class="account-button" href=link.clone() aria-label=label.clone()>
                            <Avatar url=pfp.clone() size=32/>
                        </a>
                    }
                    content=move || view! {
                        <p class="account-who">
                            <strong>{name.clone()}</strong>
                            <span class="muted">"@"{username.clone()}</span>
                        </p>
                        <a class="account-link" href=profile.clone()>"Your profile"</a>
                        <crate::components::writer::WriterMenuItem/>
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
                if let Some(root) = dialog.get_untracked() {
                    trap_tab(&root, &ev, "button, a[href]");
                }
            }
            _ => {}
        }
    });
    // Back in view (another tab, another window, the phone's own app):
    // the relay is asked at once.
    let woken = |_| {
        if !document().hidden() {
            WOKEN.with(|w| w.set(true));
        }
    };
    let shown = window_event_listener_untyped("visibilitychange", woken);
    let focused = window_event_listener_untyped("focus", woken);
    let restored = window_event_listener_untyped("pageshow", woken);
    on_cleanup(move || {
        keys.remove();
        shown.remove();
        focused.remove();
        restored.remove();
    });
    let status = move || match session.step.get() {
        Step::Starting => "Getting a sign-in code…".to_owned(),
        Step::Waiting { .. } => "Waiting for you to approve in Farcaster…".to_owned(),
        Step::Checking { who: Some(who) } => format!("@{} approved. Signing you in…", who.username),
        Step::Checking { who: None } => "Approved. Signing you in…".to_owned(),
        Step::Unconfirmed { .. } => UNCONFIRMED.to_owned(),
        Step::Failed(message) => message,
    };
    let stuck = move || matches!(session.step.get(), Step::Failed(_) | Step::Unconfirmed { .. });
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
                            <div class="qr-frame" class:failed=stuck>
                                {move || match session.step.get() {
                                    Step::Waiting { code } => view! {
                                        <svg class="qr" viewBox=format!("0 0 {0} {0}", code.size) role="img" aria-label="Sign-in QR code" shape-rendering="crispEdges">
                                            <path d=code.path/>
                                        </svg>
                                    }
                                    .into_any(),
                                    // Who approved, while the Worker checks it.
                                    Step::Checking { who: Some(who) } => view! {
                                        <span class="qr-approver" aria-hidden="true">
                                            <Avatar url=who.pfp_url size=88 large=true/>
                                            <span class="qr-approver-check">"✓"</span>
                                        </span>
                                    }
                                    .into_any(),
                                    Step::Checking { who: None } => view! { <span class="qr-check" aria-hidden="true">"✓"</span> }.into_any(),
                                    // The same approval, checked again: no new code to scan.
                                    Step::Unconfirmed { proof, who } => view! {
                                        <button type="button" class="button" on:click=move |_| recheck(session, proof.clone(), who.clone())>
                                            "Try again"
                                        </button>
                                    }
                                    .into_any(),
                                    Step::Failed(_) => view! {
                                        <button type="button" class="button" on:click=move |_| fresh(session)>"Get a new code"</button>
                                    }
                                    .into_any(),
                                    Step::Starting => view! { <span class="qr-loading" aria-hidden="true"></span> }.into_any(),
                                }}
                            </div>
                            <p class="signin-status" class:error=stuck role="status">
                                {status}
                            </p>
                            <p class="signin-note muted small">"Signing in tells Allthing who you are, nothing more. Liking, recasting and posting take a key you approve once, in the Farcaster app."</p>
                        </div>
                    </div>
                </div>
            }
        })
    }
}

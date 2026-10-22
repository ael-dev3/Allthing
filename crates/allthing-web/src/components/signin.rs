// SPDX-License-Identifier: AGPL-3.0-or-later
//! Signing in with Farcaster: the header's "Sign in" button, or the
//! signed-in account's picture and its menu, and the dialog. Where the
//! Worker endorses keys, the reader signs in by approving Allthing's key in
//! the Farcaster app, which lets them write from here too: a QR code for
//! their phone on a computer, a link to the app on a phone (`writer`'s
//! `signing_in`). The same dialog, in its approve form, is the only other
//! approval a reader meets: signed in without a key in this browser (signed
//! in before keys, after Revoke permissions, or with its storage cleared),
//! their first write asks for one (`Session::grant`, `writer`'s
//! `granting`). Elsewhere, where the Worker endorses no keys, by Sign In
//! With Farcaster: the dialog shows a QR code, polls Farcaster's relay
//! until the app has approved, and the Worker checks what the app signed
//! and sets the session cookie (`allthing_core::siwf`). Not in the static
//! copy, which has no Worker.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use allthing_core::clients::ALLTHING_APP_FID;
use allthing_core::model::UserSummary;
use allthing_core::siwf::{self, Proof, QrCode, QrRequest, RELAY, SignedIn, Started, USER_COOKIE};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::wasm_split::{SplitLoaderError, wasm_split};
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
use crate::components::writer::{Approving, Writer, granting, signing_in};
use crate::keys::Stored;
use crate::util::{GITHUB_MARK, SOURCE_URL, can_hover, document, trap_tab, user_href};

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
const UNLOADED: &str = "This part of Allthing couldn't load. Check the connection, or reload the page.";
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
    /// Signing in by approving Allthing's key, which lets the reader write
    /// from here too: the link is up, drawn as a code where a phone has to
    /// scan it.
    Approving {
        code: Option<QrCode>,
        link: String,
    },
    /// The key is approved: the Farcaster app adds it, then the Worker
    /// checks it.
    Adding,
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

/// What the dialog is for: signing in, or, for a reader signed in without a
/// key in this browser, approving Allthing to write from here.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    SignIn,
    Grant,
}

/// Who is signed in, as the Worker's user cookie says, and the sign-in
/// dialog's state. Held by the app, so a sign-in finishing after its dialog
/// closed has somewhere to land.
#[derive(Clone, Copy)]
pub struct Session {
    pub user: RwSignal<Option<UserSummary>>,
    open: RwSignal<bool>,
    step: RwSignal<Step>,
    purpose: RwSignal<Purpose>,
    /// Whether the Worker endorses keys, once a sign-in's start has said:
    /// approving Allthing's key then signs in, and lets the reader write.
    keys: RwSignal<Option<bool>>,
    /// The account the key request shown is from, as the Farcaster app
    /// names it.
    asker: RwSignal<Option<u64>>,
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
    /// The writer a grant is for (`Session::grant`): it takes the key once
    /// approved, and does the write that asked for it.
    static GRANT_FOR: Cell<Option<Writer>> = const { Cell::new(None) };
    static TABS_CHANNEL: RefCell<Option<web_sys::BroadcastChannel>> = const { RefCell::new(None) };
}

impl Session {
    pub fn new() -> Session {
        let session = Session {
            user: RwSignal::new(cookie_user()),
            open: RwSignal::new(false),
            step: RwSignal::new(Step::Starting),
            purpose: RwSignal::new(Purpose::SignIn),
            keys: RwSignal::new(None),
            asker: RwSignal::new(None),
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
            // Signed in there: this tab's sign-in is done. Signed out, or
            // someone else: its approval is for no one.
            let done = match self.purpose.get_untracked() {
                Purpose::SignIn => user.is_some(),
                Purpose::Grant => user != self.user.get_untracked(),
            };
            if done && self.open.get_untracked() {
                self.close_quietly();
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

    /// Open the sign-in dialog.
    pub fn sign_in(self) {
        if !self.open.get_untracked() {
            self.purpose.set(Purpose::SignIn);
            self.open();
        }
    }

    /// Open the dialog in its approve form, for the reader signed in without
    /// a key in this browser: the same code (or link to the Farcaster app),
    /// the same approval, which here signs no one in but lets them write
    /// from here. Once approved, the key goes to `writer`, which does the
    /// write that asked for it.
    pub fn grant(self, writer: Writer) {
        if self.user.get_untracked().is_none() {
            return;
        }
        GRANT_FOR.with(|g| g.set(Some(writer)));
        if !self.open.get_untracked() {
            self.purpose.set(Purpose::Grant);
            self.open();
        }
    }

    /// Close the dialog, stopping its sign-in or approval, and return focus
    /// to where it was.
    fn close(self) {
        self.close_quietly();
        if let Some(element) = RETURN_FOCUS.with(|r| r.borrow_mut().take()).filter(|el| el.is_connected()) {
            let _ = element.focus();
        }
    }

    /// Close the dialog: an approval not given drops the write that asked
    /// for it.
    fn close_quietly(self) {
        if let Some(writer) = GRANT_FOR.with(Cell::take) {
            writer.grant_dropped();
        }
        self.dismiss();
    }

    /// Approved: the key goes to the writer that asked, which does the
    /// write it asked for, and the dialog closes.
    fn granted(self, stored: Stored) {
        let writer = GRANT_FOR.with(Cell::take);
        self.dismiss();
        if let Some(element) = RETURN_FOCUS.with(|r| r.borrow_mut().take()).filter(|el| el.is_connected()) {
            let _ = element.focus();
        }
        if let Some(writer) = writer {
            writer.granted(stored);
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

/// Whether the user cookie names a reader: signed in, as the session says,
/// for what runs outside the app's context (an event handler's prefetch).
pub fn signed_in() -> bool {
    cookie_user().is_some()
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

/// Start a sign-in, or an approval, with a fresh code.
fn begin(session: Session) {
    let run = next_run();
    session.step.set(Step::Starting);
    session.asker.set(None);
    if session.purpose.get_untracked() == Purpose::Grant {
        let Some(fid) = session.user.with_untracked(|u| u.as_ref().map(|u| u.fid)) else { return };
        spawn_local(async move {
            match grant(run, session, fid).await {
                Ok(stored) if current(run) => session.granted(stored),
                Err(message) if current(run) && !message.is_empty() => session.step.set(Step::Failed(message)),
                _ => {}
            }
        });
        return;
    }
    spawn_local(async move {
        let outcome = match sign_in_flow(run, session).await {
            Ok(flow) => flow.await,
            Err(_) => Err(UNLOADED.to_owned()),
        };
        settle(session, run, outcome);
    });
}

/// Check an approval the Worker couldn't check before, with no new code.
fn recheck(session: Session, proof: Proof, who: Option<Approver>) {
    let run = next_run();
    spawn_local(async move {
        let outcome = match check_flow(run, session.step, proof, who).await {
            Ok(flow) => flow.await,
            Err(_) => Err(UNLOADED.to_owned()),
        };
        settle(session, run, outcome);
    });
}

/// Fetch the sign-in's code ahead of a click that opens the dialog.
fn warm() {
    spawn_local(async {
        let _ = preload_sign_in().await;
    });
}

/// A sign-in under way, to its end.
type Flow = Pin<Box<dyn Future<Output = Result<UserSummary, String>>>>;

/// A sign-in, from a code from Farcaster's relay to the Worker's check of
/// the approval: code of its own, fetched as Sign in is pointed at,
/// focused or pressed (`warm`), or when the dialog opens (`crate::split`).
#[wasm_split(signin, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_sign_in))]
fn sign_in_flow(run: u64, session: Session) -> Result<Flow, SplitLoaderError> {
    Ok(Box::pin(sign_in(run, session)))
}

/// The Worker's check of an approval again, in the sign-in's code.
#[wasm_split(signin, wasm_split_path = ::leptos::wasm_split, fallible)]
fn check_flow(run: u64, step: RwSignal<Step>, proof: Proof, who: Option<Approver>) -> Result<Flow, SplitLoaderError> {
    Ok(Box::pin(check(run, step, proof, who)))
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

async fn sign_in(run: u64, session: Session) -> Result<UserSummary, String> {
    let step = session.step;
    let failed = |failure: api::Failure| failure.message();
    let started: Started = api::post(&siwf::start_path(), &Nothing {}).await.map_err(failed)?;
    session.keys.set(Some(started.keys));
    // Where the Worker endorses keys, approving Allthing's key signs in.
    if started.keys {
        return by_key(run, session).await;
    }
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

/// A sign-in by approving Allthing's key, in writing's own code (`writer`),
/// which the dialog follows.
async fn by_key(run: u64, session: Session) -> Result<UserSummary, String> {
    match signing_in(Rc::new(move || current(run)), following(run, session)).await {
        Ok(flow) => flow.await,
        Err(_) => Err(UNLOADED.to_owned()),
    }
}

/// An approval of Allthing's key for `fid`, signed in already, in writing's
/// own code, which the dialog follows as it follows a sign-in by key.
async fn grant(run: u64, session: Session, fid: u64) -> Result<Stored, String> {
    match granting(fid, Rc::new(move || current(run)), following(run, session)).await {
        Ok(flow) => flow.await,
        Err(_) => Err(UNLOADED.to_owned()),
    }
}

/// The dialog's step, as an approval of Allthing's key goes, and who the
/// request is from.
fn following(run: u64, session: Session) -> Callback<Approving> {
    Callback::new(move |approving: Approving| {
        if current(run) {
            session.step.set(match approving {
                Approving::Waiting { code, link, asker } => {
                    session.asker.set(asker);
                    Step::Approving { code, link }
                }
                Approving::Adding => Step::Adding,
            });
        }
    })
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
            <button
                type="button"
                class="sign-in-button"
                aria-label="Sign in with Farcaster"
                on:pointerenter=move |_| warm()
                on:pointerdown=move |_| warm()
                on:focus=move |_| warm()
                on:click=move |_| session.open()
            >
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
                    // Room for Revoke permissions' explanation, under it,
                    // without scrolling, where the screen allows.
                    max_px=640.0
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
                        <crate::components::dc_key::MessagesMenuItem/>
                        <crate::components::writer::ScheduledMenuItem/>
                        <crate::components::writer::WriterMenuItem/>
                        <crate::components::dc_key::DcKeyMenuItem/>
                        <crate::miniapps::MiniAppsSwitch class="account-link"/>
                        <a class="account-link account-source" href=SOURCE_URL target="_blank" rel="noopener">
                            <svg class="github-mark" viewBox="0 0 16 16" aria-hidden="true"><path d=GITHUB_MARK/></svg>
                            "Allthing on GitHub"
                        </a>
                        <button type="button" class="account-link sign-out" on:click=move |_| session.sign_out()>"Sign out"</button>
                    }
                />
            }
            .into_any()
        }
    }
    }
}

/// The sign-in dialog: the QR code to scan with the phone's camera, or, to
/// approve Allthing's key on a phone, the link to the Farcaster app; and
/// how it's going.
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
        crate::util::dialog_open("signin-open", open);
    });
    // An approval is for whoever is signed in: signed out, it closes.
    Effect::new(move |_| {
        if session.user.with(Option::is_none)
            && session.purpose.get_untracked() == Purpose::Grant
            && session.open.get_untracked()
        {
            session.close_quietly();
        }
    });
    let for_grant = move || session.purpose.get() == Purpose::Grant;
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
        Step::Starting if for_grant() => "Getting a code…".to_owned(),
        Step::Starting => "Getting a sign-in code…".to_owned(),
        Step::Waiting { .. } | Step::Approving { .. } => "Waiting for you to approve in Farcaster…".to_owned(),
        Step::Adding if for_grant() => "Approved. Adding Allthing's key to your account…".to_owned(),
        Step::Adding => "Approved. Signing you in…".to_owned(),
        Step::Checking { who: Some(who) } => format!("@{} approved. Signing you in…", who.username),
        Step::Checking { who: None } => "Approved. Signing you in…".to_owned(),
        Step::Unconfirmed { .. } => UNCONFIRMED.to_owned(),
        Step::Failed(message) => message,
    };
    let stuck = move || matches!(session.step.get(), Step::Failed(_) | Step::Unconfirmed { .. });
    // Where the Worker endorses keys, signing in is approving Allthing's
    // key, which lets the reader write from here too (on a phone, by the
    // link to the Farcaster app, as it can't scan its own screen); an
    // approval for a reader signed in already is only ever that.
    let keyed = move || for_grant() || session.keys.get() == Some(true);
    // A phone can't scan its own screen: approving Allthing's key, it opens
    // the Farcaster app by a link instead, from the first step to the last.
    let intro = move || match (keyed(), can_hover()) {
        (true, false) => "Approve Allthing in the Farcaster app, then come back here.",
        (true, true) => "Scan the code with your phone's camera, then approve Allthing in the Farcaster app.",
        (false, _) => "Scan the code with your phone's camera, then approve in the Farcaster app.",
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
                    <div class="modal signin-modal" class:key-modal=for_grant role="dialog" aria-modal="true" aria-labelledby="signin-title" node_ref=dialog>
                        // A div: a header here would be a second banner landmark.
                        <div class="modal-head">
                            <h2 id="signin-title">
                                {move || if for_grant() { "Approve Allthing to write from here" } else { "Sign in with Farcaster" }}
                            </h2>
                            <button type="button" class="modal-close" aria-label="Close" node_ref=close_button on:click=move |_| session.close()>"×"</button>
                        </div>
                        <div class="modal-body signin-body">
                            <p class="signin-intro">{intro}</p>
                            <div
                                class="qr-frame"
                                class:failed=stuck
                                class:as-link=move || matches!(session.step.get(), Step::Approving { code: None, .. })
                            >
                                {move || match session.step.get() {
                                    Step::Waiting { code } | Step::Approving { code: Some(code), .. } => view! {
                                        <svg
                                            class="qr"
                                            viewBox=format!("0 0 {0} {0}", code.size)
                                            role="img"
                                            aria-label=if for_grant() { "QR code to approve Allthing" } else { "Sign-in QR code" }
                                            shape-rendering="crispEdges"
                                        >
                                            <path d=code.path/>
                                        </svg>
                                    }
                                    .into_any(),
                                    Step::Approving { code: None, link } => view! {
                                        <a class="button key-approve" href=link>"Approve in Farcaster"</a>
                                    }
                                    .into_any(),
                                    // The reader approving, signed in already, while their
                                    // key is added; or a check, signing in.
                                    Step::Adding if for_grant() => view! {
                                        <span class="qr-approver" aria-hidden="true">
                                            <Avatar url=session.user.with_untracked(|u| u.as_ref().and_then(|u| u.pfp_url.clone())) size=88 large=true/>
                                            <span class="qr-approver-check">"✓"</span>
                                        </span>
                                    }
                                    .into_any(),
                                    Step::Adding => view! { <span class="qr-check" aria-hidden="true">"✓"</span> }.into_any(),
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
                            <p class="signin-note muted small">
                                // What the approval allows, said before the scan; nothing
                                // until the Worker has said which way a sign-in goes.
                                {move || match (for_grant(), session.keys.get()) {
                                    (true, _) => Some("One approval lets you cast, reply, quote, like, recast, follow and delete from here, shown \u{201c}via Allthing\u{201d}. "),
                                    (false, Some(true)) => Some("One approval signs you in and lets you cast, reply, quote, like, recast, follow and delete from here, shown \u{201c}via Allthing\u{201d}. "),
                                    (false, Some(false)) => Some("Signing in tells Allthing who you are, nothing more."),
                                    (false, None) => None,
                                }}
                                // The Farcaster app names the account that asks: Allthing's
                                // own, so a reader can check it is.
                                {move || {
                                    (keyed() && session.asker.get() == Some(ALLTHING_APP_FID))
                                        .then_some("The Farcaster app shows the request as from Allthing, @allthingapp. ")
                                }}
                                {move || {
                                    keyed().then_some(
                                        "Allthing's key acts only when you press a button. It stays in this browser, and Allthing's servers never see it. Signing out deletes it.",
                                    )
                                }}
                            </p>
                        </div>
                    </div>
                </div>
            }
        })
    }
}

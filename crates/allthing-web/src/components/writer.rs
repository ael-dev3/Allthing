// SPDX-License-Identifier: AGPL-3.0-or-later
//! Liking and recasting from Allthing. A signed-in reader approves an app
//! key once, by QR code in their Farcaster app (`allthing_core::app_key`);
//! this browser keeps the key (`crate::keys`), signs each like or recast
//! with it, and hands the message to the node (`allthing_core::message`).
//!
//! Here: whether the reader can ([`Writer`]), the like and recast buttons
//! that `Engagement` shows on every cast, the dialog with the code, what
//! the reader liked and recasted (their latest of each, from the node, and
//! what they do here), and a short notice when something fails. All of it
//! waits until the Worker endorses keys (`/api/v1/signer`), which takes an
//! account of Allthing's own; until then, and for a reader signed out, each
//! write goes to the Farcaster app instead (`components::handoff`). Not in
//! the static copy, which has no sign-in.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use allthing_core::app_key::{
    self, Endorse, KeyState, Writes, endorse_path, key_request_url, key_requests_url, parse_requested, writes_path,
};
use allthing_core::clients::ALLTHING_APP_FID;
use allthing_core::compose::{cast_url, profile_url};
use allthing_core::emoji::Emoji;
use allthing_core::message::{self, Body, CastId, MessageData, NewCast, Parent, Reaction, Sent, Submitted};
use allthing_core::provenance::{base64_encode, hex_encode};
use allthing_core::scheduled::{SCHEDULED_PATH, Schedule, ScheduledCast};
use allthing_core::siwf::{self, QrCode, QrRequest};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlElement;

use crate::api;
use crate::components::chips::Avatar;
use crate::components::handoff::{Ask, Dismissed, HandOff, use_hand_off};
use crate::components::signin::{Session, use_session};
use crate::keys::{self, KeyError, Stored};
use crate::util::{can_hover, cast_href, document, local_time, scheduled_href, trap_tab};

/// How often the dialog asks the Farcaster app's API about the request (as
/// Farcaster's own example does), and how often the node is asked whether
/// it has the new key yet, at most.
const POLL_MS: i32 = 2000;
const NODE_CHECKS: u32 = 15;
/// How many of the reader's latest likes, and recasts, show as theirs.
const RECENT: u32 = 100;
/// A key approved this recently may not have reached the node yet: a
/// message it refuses as unknown is sent again, after 2, 4 and 8 seconds.
const FRESH_MS: f64 = 300_000.0;
const RETRIES: u32 = 3;
const RETRY_MS: i32 = 2000;
/// Requests to the node or the Farcaster app's API give up after this.
const TIMEOUT_MS: i32 = 15_000;
/// How long a notice stays.
const NOTICE_MS: i32 = 6000;
/// After a write goes to the Farcaster app, how often the node is asked
/// whether the reader did it there (and at once when the page is back in
/// view): every 3 s for a minute, then every 10 s; and for how long. A
/// cast takes longer to write there than a like takes to tap.
const WATCH_MS: i32 = 3000;
const WATCH_SLOW_MS: i32 = 10_000;
const SLOW_AFTER_MS: f64 = 60_000.0;
const WATCH_FOR_MS: f64 = 180_000.0;
const POSTING_FOR_MS: f64 = 600_000.0;
/// The site's other tabs hear on this when a key is approved or forgotten.
const TABS: &str = "allthing-keys";
/// The casts the reader deleted, kept for the tab: a page read from a cache
/// may still have one for a while.
const DELETED: &str = "allthing-deleted";

const EXPIRED: &str = "This code has expired. Get a new one to approve Allthing.";
const FARCASTER_DOWN: &str = "Couldn't reach the Farcaster app's API. Check your connection and try again.";
const NODE_DOWN: &str = "Couldn't reach Farcaster's network. Check your connection and try again.";
const UNSUPPORTED: &str = "This browser can't keep a signing key safely. Update it, or use another browser, to like and recast from Allthing.";
const KEY_GONE: &str =
    "Allthing's key isn't on your Farcaster account anymore. Like or recast again to approve a new one.";

/// Whether the reader can like and recast here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Can {
    /// Signed out, or the Worker endorses no keys: writes go to the
    /// Farcaster app.
    No,
    /// Being found out.
    Checking,
    /// Once they approve a key: a like or a recast opens the dialog.
    Approve,
    /// Yes: the key signs.
    Yes,
}

/// Where an approval stands, in the dialog.
#[derive(Clone, PartialEq)]
enum Step {
    /// Making the key, and the request, and the code.
    Starting,
    /// The link is up, drawn as a code where a phone has to scan it; the
    /// Farcaster app hasn't answered yet.
    Waiting {
        code: Option<QrCode>,
        link: String,
    },
    /// Approved: the Farcaster app is adding the key to the account.
    Adding,
    Failed(String),
}

/// What the reader liked and recasted, by cast hash and kind.
#[derive(Clone, Default)]
struct Mine {
    /// As the node says: their latest of each kind, and casts checked one
    /// by one.
    known: HashSet<(String, Reaction)>,
    /// What they did here this visit: `true` added, `false` taken back.
    changed: HashMap<(String, Reaction), bool>,
    /// Being sent.
    busy: HashSet<(String, Reaction)>,
    /// Whom the reader follows, as the node says, by fid.
    follows: HashMap<u64, bool>,
    /// Follows and unfollows this visit, and those being sent.
    follow_changed: HashMap<u64, bool>,
    follow_busy: HashSet<u64>,
    /// Casts the reader deleted this visit, and those being deleted.
    deleted: HashSet<String>,
    deleting: HashSet<String>,
    /// The emoji the reader reacted to casts with this visit, by cast
    /// hash, and the casts a reaction is being sent to.
    emoji: HashMap<String, Vec<&'static Emoji>>,
    reacting: HashSet<String>,
    /// The reply each of those is, once it's known, by cast hash and
    /// emoji: what taking it back deletes.
    reaction_replies: HashMap<(String, &'static str), String>,
    /// Writes handed to the Farcaster app that show here before the node
    /// has them, by what they are about (`Expect::about`).
    pending: HashSet<String>,
    /// Casts handed to the Farcaster app this visit, oldest first.
    outgoing: Vec<Outgoing>,
}

impl Mine {
    fn reacted(&self, key: &(String, Reaction)) -> bool {
        self.changed.get(key).copied().unwrap_or_else(|| self.known.contains(key))
    }

    /// How much the cast's count, read before, is off by now.
    fn delta(&self, key: &(String, Reaction)) -> i64 {
        match (self.changed.get(key), self.known.contains(key)) {
            (Some(true), false) => 1,
            (Some(false), true) => -1,
            _ => 0,
        }
    }

    /// Whether the reader follows `fid`, once known.
    fn following(&self, fid: u64) -> Option<bool> {
        self.follow_changed.get(&fid).or_else(|| self.follows.get(&fid)).copied()
    }
}

/// A short word for the reader, with a link to follow, perhaps.
#[derive(Clone, PartialEq)]
struct Notice {
    text: String,
    link: Option<(String, &'static str)>,
}

/// What a write handed to the Farcaster app asks the reader to do there.
/// Signed in, it shows here at once, as theirs but not done yet
/// (`pending`), and as done once the node has it.
#[derive(Clone)]
pub enum Expect {
    /// Their like or recast of `target`, now `on`.
    Reaction { target: CastId, kind: Reaction, on: bool },
    /// Their follow of `fid`, now `on`.
    Follow { fid: u64, on: bool },
    /// One of their casts, gone: shown once it is, not before.
    Deleted { target: CastId },
    /// Their emoji reaction to `target`: a reply that is the emoji.
    Reacted { target: CastId, emoji: &'static Emoji },
    /// A cast, a reply or a quote. `settle` hears whether it was posted,
    /// for the composer to let the draft go, or to have it back.
    Posted { cast: Outgoing, settle: Option<Settle> },
}

/// What the composer does once its draft, handed to the Farcaster app, is
/// posted there (`true`), or isn't in time (`false`). Not a `Callback`: the
/// composer that made it may be gone by then.
pub type Settle = std::sync::Arc<dyn Fn(bool) + Send + Sync>;

impl Expect {
    /// What it is about: a newer hand-off of the same takes its place.
    fn about(&self) -> String {
        match self {
            Expect::Reaction { target, kind, .. } => format!("{kind:?} {}", hex_encode(&target.hash)),
            Expect::Follow { fid, .. } => format!("follow {fid}"),
            Expect::Deleted { target } => format!("delete {}", hex_encode(&target.hash)),
            Expect::Reacted { target, emoji } => format!("react {} {}", hex_encode(&target.hash), emoji.code),
            Expect::Posted { cast, .. } => format!("post {}", cast.id),
        }
    }
}

/// A cast handed to the Farcaster app to post, shown here as the reader's
/// at once (`components::outgoing`), and as posted once the node has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Outgoing {
    /// Which hand-off it is.
    pub id: u64,
    pub sent: Sent,
    pub text: String,
    /// The GIF it embeds, as its tile.
    pub image: Option<String>,
    /// Whose cast it quotes, or replies to.
    pub quoting: Option<String>,
    pub replying_to: Option<String>,
    /// The channel it goes to.
    pub channel: Option<String>,
    /// For a reply, its cast's reply count when it went.
    pub replies_before: u64,
    /// Its hash, once the node has it.
    pub hash: Option<String>,
}

/// How to take back what a hand-off shows before it's done.
enum Undo {
    Reaction { key: (String, Reaction), before: Option<bool> },
    Follow { fid: u64, before: Option<bool> },
    Reacted { hash: String, emoji: &'static Emoji },
    Posted { id: u64, settle: Option<Settle> },
}

/// What the node says of a hand-off: not yet, or done there (with the
/// cast it posted, for a cast).
enum Landed {
    No,
    Yes(Option<String>),
}

/// The reader's likes, recasts and casts, held by the app.
#[derive(Clone, Copy)]
pub struct Writer {
    session: Session,
    hand_off: HandOff,
    can: RwSignal<Can>,
    /// The account the Worker asks for readers' keys as, once it has said.
    app: RwSignal<Option<u64>>,
    open: RwSignal<bool>,
    step: RwSignal<Step>,
    mine: RwSignal<Mine>,
    notice: RwSignal<Option<Notice>>,
    /// Moves on whenever the reader schedules, changes or cancels a cast,
    /// for their list to be read again.
    scheduled: RwSignal<u32>,
}

/// A write waiting for the check, and the hand-off it becomes without a
/// key, with what to look for after.
type Pending = (Box<dyn FnOnce()>, Ask, Option<Expect>);

thread_local! {
    /// The reader's approved key.
    static KEY: RefCell<Option<Stored>> = const { RefCell::new(None) };
    /// The approval under way: closing the dialog moves this on, and the
    /// older one stops polling.
    static RUN: Cell<u64> = const { Cell::new(0) };
    /// Which check of the reader's state is current.
    static CHECK: Cell<u64> = const { Cell::new(0) };
    /// What opened the dialog (a like, a recast, a cast): done once the
    /// key is approved.
    static WAITING: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    /// A write asked for before the check was back: done once it is (after
    /// approving a key, if the reader needs one), or handed to the
    /// Farcaster app if Allthing doesn't write for them.
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
    /// Usernames looked up for mentions this visit, and whose they are.
    static NAMES: RefCell<HashMap<String, Option<u64>>> = RefCell::new(HashMap::new());
    static RETURN_FOCUS: RefCell<Option<HtmlElement>> = const { RefCell::new(None) };
    /// The last timestamp each like or recast was sent with: a message that
    /// undoes another goes out at least a second later, so it wins.
    static SENT_AT: RefCell<HashMap<String, i64>> = RefCell::new(HashMap::new());
    /// Moves on when the page comes back into view: every poll waiting
    /// then asks at once.
    static WAKES: Cell<u64> = const { Cell::new(0) };
    static NOTICE_RUN: Cell<u64> = const { Cell::new(0) };
    static TABS_CHANNEL: RefCell<Option<web_sys::BroadcastChannel>> = const { RefCell::new(None) };
    /// The hand-offs being looked for on the node, by what they are about,
    /// and the run each is.
    static WATCHES: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    static WATCH_RUN: Cell<u64> = const { Cell::new(0) };
    /// How to take back what each shows until it lands, by what it is about,
    /// with the run that showed it.
    static UNDO: RefCell<HashMap<String, (u64, Undo)>> = RefCell::new(HashMap::new());
}

pub fn use_writer() -> Option<Writer> {
    use_context::<Writer>()
}

impl Writer {
    /// The app's: it follows who is signed in.
    pub fn new() -> Writer {
        let session = use_session();
        let writer = Writer {
            session,
            hand_off: use_hand_off(),
            can: RwSignal::new(Can::No),
            app: RwSignal::new(None),
            open: RwSignal::new(false),
            step: RwSignal::new(Step::Starting),
            mine: RwSignal::new(Mine { deleted: deleted_before(), ..Mine::default() }),
            notice: RwSignal::new(None),
            scheduled: RwSignal::new(0),
        };
        Effect::new(move |before: Option<Option<u64>>| {
            let fid = session.user.with(|u| u.as_ref().map(|u| u.fid));
            if before.as_ref() != Some(&fid) {
                match fid {
                    Some(fid) => writer.check(fid),
                    None => writer.signed_out(before.flatten()),
                }
            }
            fid
        });
        writer.follow_tabs();
        writer
    }

    /// Whether the reader can write from here, with a key they have or will
    /// approve: signed in, where the Worker endorses keys (their scheduled
    /// casts).
    pub fn can_write(self) -> bool {
        matches!(self.can.get(), Can::Approve | Can::Yes)
    }

    /// Whether the reader's writes go to the Farcaster app: signed out, or
    /// where the Worker endorses no keys. A button then opens the hand-off,
    /// and isn't a toggle.
    pub fn hands_off(self) -> bool {
        self.can.get() == Can::No
    }

    /// Whether what the reader liked and recasted shows: signed in, once
    /// it's known whether Allthing writes for them (with a key, or in the
    /// Farcaster app).
    pub fn shows_mine(self) -> bool {
        self.reader().is_some() && matches!(self.can.get(), Can::Yes | Can::No)
    }

    /// Whether the reader likes, or recasted, `hash`, as far as is known.
    pub fn reacted(self, hash: &str, kind: Reaction) -> bool {
        self.mine.with(|m| m.reacted(&(hash.to_owned(), kind)))
    }

    /// How far the count of `kind` read with `hash` is off, after what the
    /// reader did here.
    pub fn delta(self, hash: &str, kind: Reaction) -> i64 {
        self.mine.with(|m| m.delta(&(hash.to_owned(), kind)))
    }

    pub fn busy(self, hash: &str, kind: Reaction) -> bool {
        self.mine.with(|m| m.busy.contains(&(hash.to_owned(), kind)))
    }

    /// Like or recast `target`, or take it back: at once with the reader's
    /// key, or once they have approved one; or in the Farcaster app, as
    /// `ask` says (`when_ready`).
    pub fn toggle(self, target: CastId, kind: Reaction, ask: Ask) {
        let key = (hex_encode(&target.hash), kind);
        if self.mine.with_untracked(|m| m.busy.contains(&key)) {
            return;
        }
        let on = !self.mine.with_untracked(|m| m.reacted(&key));
        if self.can.get_untracked() != Can::Yes {
            let again = ask.clone();
            let expect = Expect::Reaction { target, kind, on };
            self.when_ready(move || self.toggle(target, kind, again), ask, Some(expect));
            return;
        }
        let Some(stored) = KEY.with(|k| k.borrow().clone()) else { return };
        let before = self.mine.with_untracked(|m| m.changed.get(&key).copied());
        self.mine.update(|m| {
            m.changed.insert(key.clone(), on);
            m.busy.insert(key.clone());
        });
        spawn_local(async move {
            let body = if on { Body::ReactionAdd(kind, target) } else { Body::ReactionRemove(kind, target) };
            let outcome = send(&stored, body, Some(&format!("{kind:?} {}", key.0))).await;
            self.mine.update(|m| {
                m.busy.remove(&key);
                if outcome.is_err() {
                    match before {
                        Some(was) => m.changed.insert(key.clone(), was),
                        None => m.changed.remove(&key),
                    };
                }
            });
            match outcome {
                Ok(_) => {}
                Err(Unsent::KeyGone) => {
                    self.forget_key(stored.fid);
                    self.say(KEY_GONE.to_owned());
                }
                Err(Unsent::Failed(message)) => self.say(message),
            }
        });
    }

    /// Whether the reader's casts go out from here, with a key they have or
    /// will approve: yes where the Worker endorses keys, no where it
    /// doesn't, and not yet known while that is found out.
    pub fn posts_here(self) -> Option<bool> {
        match self.can.get() {
            Can::Approve | Can::Yes => Some(true),
            Can::No => Some(false),
            Can::Checking => None,
        }
    }

    /// Run `then` with the reader's key: at once, or when they have
    /// approved one, for which the dialog opens; while the Worker is asked,
    /// it waits for the answer. Signed out, or where the Worker endorses no
    /// keys, the reader does it in the Farcaster app: `ask` opens there, and,
    /// signed in, what it asks for (`expect`) shows here once they have.
    pub fn when_ready(self, then: impl FnOnce() + 'static, ask: Ask, expect: Option<Expect>) {
        match self.can.get_untracked() {
            Can::Yes => then(),
            Can::Approve => {
                WAITING.with(|w| *w.borrow_mut() = Some(Box::new(then)));
                self.connect();
            }
            Can::Checking => PENDING.with(|p| *p.borrow_mut() = Some((Box::new(then), ask, expect))),
            Can::No => self.hand_off(ask, expect),
        }
    }

    /// Hand a write to the Farcaster app: the dialog opens with `ask`, and,
    /// signed in, what it asks for (`expect`) shows here at once, as theirs
    /// but not done yet, until the node has it. Closing the dialog without
    /// going to the app takes it back; it still shows if it lands.
    pub fn hand_off(self, ask: Ask, expect: Option<Expect>) {
        match expect.filter(|_| self.fid().is_some()) {
            Some(expect) => {
                let (about, run) = self.went(expect, ask.url.clone());
                let dismissed: Dismissed = std::sync::Arc::new(move || self.take_back(&about, run));
                self.hand_off.open(Ask { dismissed: Some(dismissed), ..ask });
            }
            None => self.hand_off.open(ask),
        }
    }

    /// A write that went to the Farcaster app by a link of its own, as a
    /// phone's "Post in Farcaster" does: as `hand_off`, without the dialog.
    pub fn went_to_app(self, expect: Expect, url: String) {
        if self.fid().is_some() {
            self.went(expect, url);
        }
    }

    /// Post `cast` with the reader's key: its hash, or why it didn't go out.
    pub async fn cast(self, cast: NewCast) -> Result<String, String> {
        self.cast_in(cast, None).await
    }

    /// Post `cast`, as [`Writer::cast`]; one sent in the same `slot` as
    /// another goes out at least a second after it (`send`).
    async fn cast_in(self, cast: NewCast, slot: Option<String>) -> Result<String, String> {
        let Some(stored) = KEY.with(|k| k.borrow().clone()) else { return Err(KEY_GONE.into()) };
        match send(&stored, Body::CastAdd(cast), slot.as_ref()).await {
            Ok(hash) => Ok(hex_encode(&hash)),
            Err(Unsent::KeyGone) => {
                self.forget_key(stored.fid);
                Err(KEY_GONE.into())
            }
            Err(Unsent::Failed(message)) => Err(message),
        }
    }

    /// Sign `cast` with the reader's key for `at` (Unix seconds), its
    /// timestamp, and hand it to the Worker to give the node then
    /// (`allthing_core::scheduled`); with `replaces`, in the place of one of
    /// theirs. `draft` is what the composer had, kept for their list. Says
    /// so when it's kept; otherwise, why not.
    pub async fn schedule(
        self,
        cast: NewCast,
        at: i64,
        draft: String,
        replaces: Option<i64>,
    ) -> Result<ScheduledCast, String> {
        let Some(stored) = KEY.with(|k| k.borrow().clone()) else { return Err(KEY_GONE.into()) };
        let signer = app_key::parse_public_key(&stored.key).ok_or_else(|| KEY_GONE.to_owned())?;
        let data = MessageData::new(stored.fid, at, Body::CastAdd(cast)).encode();
        let hash = message::message_hash(&data);
        let signature = keys::sign(&stored, &hash).await.map_err(key_message)?;
        let message = base64_encode(&message::envelope(&data, &hash, &signature, &signer));
        let kept = api::post::<_, ScheduledCast>(SCHEDULED_PATH, &Schedule { message, draft, replaces })
            .await
            .map_err(|failure| failure.message())?;
        let text = if replaces.is_some() { "Changed: it goes out" } else { "Scheduled: it goes out" };
        self.notify(Notice {
            text: format!("{text} {}.", local_time(kept.at)),
            link: Some((scheduled_href(), "View")),
        });
        self.rescheduled();
        Ok(kept)
    }

    /// The reader's scheduled casts changed: their list is read again.
    pub fn rescheduled(self) {
        self.scheduled.update(|n| *n += 1);
    }

    /// Which change to the reader's scheduled casts the list is at.
    pub fn scheduled_version(self) -> u32 {
        self.scheduled.get()
    }

    /// React to `target` with `emoji`: a reply that is the emoji and
    /// nothing else, since Farcaster's own reactions are only likes and
    /// recasts. With the reader's key, at once, or once they have approved
    /// one; or in the Farcaster app, as `ask` says.
    pub fn react(self, target: CastId, emoji: &'static Emoji, ask: Ask) {
        let hash = hex_encode(&target.hash);
        if self.mine.with_untracked(|m| m.reacting.contains(&hash)) {
            return;
        }
        if self.can.get_untracked() != Can::Yes {
            let again = ask.clone();
            self.when_ready(move || self.react(target, emoji, again), ask, Some(Expect::Reacted { target, emoji }));
            return;
        }
        self.mine.update(|m| {
            m.reacting.insert(hash.clone());
        });
        spawn_local(async move {
            let reply =
                NewCast { text: format!(":{}:", emoji.code), parent: Some(Parent::Cast(target)), ..NewCast::default() };
            // Each in a slot of its own: a reaction taken back and made
            // again goes out a second later at least, so it's a new reply
            // and not the one deleted, which the node won't take again.
            let outcome = self.cast_in(reply, Some(format!("react {hash} {}", emoji.code))).await;
            self.mine.update(|m| {
                m.reacting.remove(&hash);
                if let Ok(reply) = &outcome {
                    m.emoji.entry(hash.clone()).or_default().push(emoji);
                    m.reaction_replies.insert((hash.clone(), emoji.code), reply.clone());
                }
            });
            match outcome {
                Ok(reply) => self.notify(Notice {
                    text: format!("Reacted with :{}:, as a reply.", emoji.code),
                    link: Some((cast_href(&reply), "View")),
                }),
                Err(message) => self.say(message),
            }
        });
    }

    /// The emoji the reader reacted to `hash` with this visit.
    pub fn reactions(self, hash: &str) -> Vec<&'static Emoji> {
        self.mine.with(|m| m.emoji.get(hash).cloned().unwrap_or_default())
    }

    /// The reply that is the reader's reaction to `hash` with `emoji` this
    /// visit, once it's known.
    pub fn reaction_reply(self, hash: &str, emoji: &Emoji) -> Option<String> {
        self.mine.with(|m| m.reaction_replies.get(&(hash.to_owned(), emoji.code)).cloned())
    }

    /// Whether a reaction to `hash` is being sent.
    pub fn reacting(self, hash: &str) -> bool {
        self.mine.with(|m| m.reacting.contains(hash))
    }

    /// Say that a cast went out, with a link to it.
    pub fn posted(self, hash: &str) {
        self.notify(Notice { text: "Posted.".into(), link: Some((cast_href(hash), "View")) });
    }

    /// Who is signed in, as the page follows it.
    pub fn reader(self) -> Option<u64> {
        self.session.user.with(|u| u.as_ref().map(|u| u.fid))
    }

    /// Whether the reader follows `fid`: `None` until the node has said.
    pub fn follows(self, fid: u64) -> Option<bool> {
        self.mine.with(|m| m.following(fid))
    }

    /// How far `fid`'s follower count, read before, is off now.
    pub fn followers_delta(self, fid: u64) -> i64 {
        self.mine.with(|m| match (m.follow_changed.get(&fid), m.follows.get(&fid)) {
            (Some(true), Some(false) | None) => 1,
            (Some(false), Some(true)) => -1,
            _ => 0,
        })
    }

    pub fn follow_busy(self, fid: u64) -> bool {
        self.mine.with(|m| m.follow_busy.contains(&fid))
    }

    /// Ask the node whether the reader follows `fid`: a public read, so it
    /// needs no key.
    pub fn look_up_follow(self, fid: u64) {
        let Some(reader) = self.fid() else { return };
        spawn_local(async move {
            let Ok((status, body)) = fetch("GET", &message::link_url(reader, fid), None).await else { return };
            if let Some(found) = message::follows(status, &body)
                && self.fid() == Some(reader)
            {
                self.mine.update(|m| {
                    m.follows.insert(fid, found);
                });
            }
        });
    }

    /// Follow `fid`, or unfollow them; or in the Farcaster app, as `ask`
    /// says.
    pub fn toggle_follow(self, fid: u64, ask: Ask) {
        if self.mine.with_untracked(|m| m.follow_busy.contains(&fid)) {
            return;
        }
        if self.can.get_untracked() != Can::Yes {
            let again = ask.clone();
            let on = !self.mine.with_untracked(|m| m.following(fid).unwrap_or(false));
            self.when_ready(move || self.toggle_follow(fid, again), ask, Some(Expect::Follow { fid, on }));
            return;
        }
        let Some(stored) = KEY.with(|k| k.borrow().clone()) else { return };
        let before = self.mine.with_untracked(|m| m.follow_changed.get(&fid).copied());
        let on = !self.mine.with_untracked(|m| m.following(fid).unwrap_or(false));
        self.mine.update(|m| {
            m.follow_changed.insert(fid, on);
            m.follow_busy.insert(fid);
        });
        spawn_local(async move {
            let body = if on { Body::Follow(fid) } else { Body::Unfollow(fid) };
            let outcome = send(&stored, body, Some(&format!("follow {fid}"))).await;
            self.mine.update(|m| {
                m.follow_busy.remove(&fid);
                if outcome.is_err() {
                    match before {
                        Some(was) => m.follow_changed.insert(fid, was),
                        None => m.follow_changed.remove(&fid),
                    };
                }
            });
            match outcome {
                Ok(_) => {}
                Err(Unsent::KeyGone) => {
                    self.forget_key(stored.fid);
                    self.say(KEY_GONE.to_owned());
                }
                Err(Unsent::Failed(message)) => self.say(message),
            }
        });
    }

    /// Whether the reader deleted `hash` this visit.
    pub fn deleted(self, hash: &str) -> bool {
        self.mine.with(|m| m.deleted.contains(hash))
    }

    /// Whether `hash` is being deleted.
    pub fn deleting(self, hash: &str) -> bool {
        self.mine.with(|m| m.deleting.contains(hash))
    }

    /// Delete one of the reader's casts: once it is gone, the page shows so,
    /// and a notice says it. Or in the Farcaster app, as `ask` says.
    pub fn delete(self, hash: String, ask: Ask) {
        if self.can.get_untracked() != Can::Yes {
            let expect = self.fid().and_then(|fid| CastId::new(fid, &hash)).map(|target| Expect::Deleted { target });
            let again = ask.clone();
            self.when_ready(move || self.delete(hash, again), ask, expect);
            return;
        }
        let Some(stored) = KEY.with(|k| k.borrow().clone()) else { return };
        let Some(target) = CastId::new(stored.fid, &hash) else { return };
        if !self.mine.try_update(|m| m.deleting.insert(hash.clone())).unwrap_or(false) {
            return;
        }
        spawn_local(async move {
            let outcome = send(&stored, Body::CastRemove(target.hash), None).await;
            self.mine.update(|m| {
                m.deleting.remove(&hash);
            });
            match outcome {
                Ok(_) => {
                    self.mine.update(|m| {
                        m.deleted.insert(hash);
                        remember_deleted(&m.deleted);
                    });
                    self.say("Deleted. It leaves Farcaster as the network catches up.".into());
                }
                Err(Unsent::KeyGone) => {
                    self.forget_key(stored.fid);
                    self.say(KEY_GONE.to_owned());
                }
                Err(Unsent::Failed(message)) => self.say(message),
            }
        });
    }

    /// Ask the node whether the reader liked and recasted `target` (a
    /// post's own cast, which may be older than their latest).
    pub fn look_up(self, target: CastId) {
        let Some(fid) = self.fid() else { return };
        let hash = hex_encode(&target.hash);
        for kind in [Reaction::Like, Reaction::Recast] {
            let hash = hash.clone();
            spawn_local(async move {
                let Ok((status, body)) = fetch("GET", &message::reaction_url(fid, kind, &target), None).await else {
                    return;
                };
                let Some(found) = message::reaction_found(status, &body) else { return };
                if self.fid() == Some(fid) {
                    self.mine.update(|m| {
                        if found {
                            m.known.insert((hash, kind));
                        } else {
                            m.known.remove(&(hash, kind));
                        }
                    });
                }
            });
        }
    }

    /// Show `expect` at once as the reader's, not done yet, and look for it
    /// on the node: what it is about, and the run it is.
    fn went(self, mut expect: Expect, url: String) -> (String, u64) {
        let run = WATCH_RUN.with(|r| {
            r.set(r.get() + 1);
            r.get()
        });
        if let Expect::Posted { cast, .. } = &mut expect {
            cast.id = run;
        }
        let about = expect.about();
        // A newer hand-off of the same, taken back, goes back to where the
        // first began.
        let earlier = UNDO.with(|u| u.borrow_mut().remove(&about)).map(|(_, undo)| undo);
        let undo = self
            .mine
            .try_update(|m| {
                let undo = match &expect {
                    Expect::Reaction { target, kind, on } => {
                        let key = (hex_encode(&target.hash), *kind);
                        let before = m.changed.insert(key.clone(), *on);
                        Some(Undo::Reaction { key, before })
                    }
                    Expect::Follow { fid, on } => {
                        let before = m.follow_changed.insert(*fid, *on);
                        Some(Undo::Follow { fid: *fid, before })
                    }
                    Expect::Deleted { .. } => None,
                    Expect::Reacted { target, emoji } => {
                        let hash = hex_encode(&target.hash);
                        let reacted = m.emoji.entry(hash.clone()).or_default();
                        (!reacted.contains(emoji)).then(|| {
                            reacted.push(emoji);
                            Undo::Reacted { hash, emoji }
                        })
                    }
                    Expect::Posted { cast, settle } => {
                        m.outgoing.push(cast.clone());
                        Some(Undo::Posted { id: cast.id, settle: settle.clone() })
                    }
                };
                if undo.is_some() {
                    m.pending.insert(about.clone());
                }
                undo
            })
            .flatten();
        if let Some(undo) = earlier.or(undo) {
            UNDO.with(|u| u.borrow_mut().insert(about.clone(), (run, undo)));
        }
        self.watch(expect, url, run);
        (about, run)
    }

    /// Take back what a hand-off shows, if it hasn't landed: closed without
    /// going to the app, or not done there in time.
    fn take_back(self, about: &str, run: u64) {
        let Some(undo) = UNDO.with(|u| {
            let mut undos = u.borrow_mut();
            if undos.get(about).is_some_and(|(by, _)| *by == run) {
                undos.remove(about).map(|(_, undo)| undo)
            } else {
                None
            }
        }) else {
            return;
        };
        let mut settle = None;
        self.mine.update(|m| {
            m.pending.remove(about);
            match undo {
                Undo::Reaction { key, before } => {
                    match before {
                        Some(was) => m.changed.insert(key, was),
                        None => m.changed.remove(&key),
                    };
                }
                Undo::Follow { fid, before } => {
                    match before {
                        Some(was) => m.follow_changed.insert(fid, was),
                        None => m.follow_changed.remove(&fid),
                    };
                }
                Undo::Reacted { hash, emoji } => {
                    if let Some(reacted) = m.emoji.get_mut(&hash) {
                        reacted.retain(|e| *e != emoji);
                    }
                }
                Undo::Posted { id, settle: then } => {
                    m.outgoing.retain(|cast| cast.id != id);
                    settle = then;
                }
            }
        });
        if let Some(settle) = settle {
            settle(false);
        }
    }

    /// Look for a write that went to the Farcaster app (`url`) on the node:
    /// at once, and then every few seconds and when the page is back in
    /// view. Once it's there, it shows as done, the dialog closes if it's
    /// still open, and a notice says so. Found at once, before the reader
    /// can have done anything, a like or a follow was so already, and the
    /// page was behind (a like older than their latest). A newer hand-off of
    /// the same takes its place; signing out ends it; not done in time, what
    /// shows is taken back.
    fn watch(self, expect: Expect, url: String, run: u64) {
        let Some(reader) = self.fid() else { return };
        let about = expect.about();
        WATCHES.with(|w| w.borrow_mut().insert(about.clone(), run));
        let current =
            move |about: &str| WATCHES.with(|w| w.borrow().get(about) == Some(&run)) && self.fid() == Some(reader);
        let started = js_sys::Date::now();
        let since = (started / 1000.0) as i64;
        let window = if matches!(expect, Expect::Posted { .. }) { POSTING_FOR_MS } else { WATCH_FOR_MS };
        spawn_local(async move {
            let mut first = true;
            let mut landed = false;
            while current(&about) && js_sys::Date::now() - started < window {
                if !first {
                    pause(if js_sys::Date::now() - started < SLOW_AFTER_MS { WATCH_MS } else { WATCH_SLOW_MS }).await;
                }
                if !current(&about) {
                    break;
                }
                if let Some(Landed::Yes(hash)) = self.landed(reader, &expect, since).await
                    && current(&about)
                {
                    UNDO.with(|u| u.borrow_mut().remove(&about));
                    let already = first && matches!(expect, Expect::Reaction { .. } | Expect::Follow { .. });
                    if already {
                        self.was_so(&expect, &about)
                    } else {
                        self.show(&expect, &about, hash)
                    }
                    self.hand_off.close_if(&url);
                    landed = true;
                    break;
                }
                first = false;
            }
            if !landed && current(&about) {
                self.take_back(&about, run);
            }
            WATCHES.with(|w| {
                let mut watches = w.borrow_mut();
                if watches.get(&about) == Some(&run) {
                    watches.remove(&about);
                }
            });
        });
    }

    /// Whether the node has what `expect` asks for from `reader`, posted
    /// since `since` (Unix seconds) for a cast: `None` when it couldn't say.
    async fn landed(self, reader: u64, expect: &Expect, since: i64) -> Option<Landed> {
        let yes = |done: bool| if done { Landed::Yes(None) } else { Landed::No };
        let posted = |sent: Sent| async move {
            let (status, body) = fetch("GET", &message::casts_url(reader), None).await.ok()?;
            let casts = message::latest_casts(status, &body)?;
            Some(match sent.found(&casts, since) {
                Some(cast) => Landed::Yes(Some(cast.hash.clone())),
                None => Landed::No,
            })
        };
        match expect {
            Expect::Reaction { target, kind, on } => {
                let (status, body) = fetch("GET", &message::reaction_url(reader, *kind, target), None).await.ok()?;
                message::reaction_found(status, &body).map(|found| yes(found == *on))
            }
            Expect::Follow { fid, on } => {
                let (status, body) = fetch("GET", &message::link_url(reader, *fid), None).await.ok()?;
                message::follows(status, &body).map(|found| yes(found == *on))
            }
            Expect::Deleted { target } => {
                let (status, body) = fetch("GET", &message::cast_url(target), None).await.ok()?;
                message::cast_gone(status, &body).map(yes)
            }
            Expect::Reacted { target, emoji } => {
                posted(Sent::Reaction { parent: hex_encode(&target.hash), code: emoji.code.to_owned() }).await
            }
            Expect::Posted { cast, .. } => posted(cast.sent.clone()).await,
        }
    }

    /// Show what the node had before the hand-off, as it says: the button
    /// stays turned, and the count, which had it, stays as it was.
    fn was_so(self, expect: &Expect, about: &str) {
        self.mine.update(|m| {
            m.pending.remove(about);
            match expect {
                Expect::Reaction { target, kind, on } => {
                    let key = (hex_encode(&target.hash), *kind);
                    m.changed.remove(&key);
                    if *on {
                        m.known.insert(key);
                    } else {
                        m.known.remove(&key);
                    }
                }
                Expect::Follow { fid, on } => {
                    m.follow_changed.remove(fid);
                    m.follows.insert(*fid, *on);
                }
                Expect::Deleted { .. } | Expect::Reacted { .. } | Expect::Posted { .. } => {}
            }
        });
        self.say("Already done.".to_owned());
    }

    /// Show what the reader did in the Farcaster app as done: `hash` is the
    /// cast it posted, for a cast.
    fn show(self, expect: &Expect, about: &str, hash: Option<String>) {
        self.mine.update(|m| {
            m.pending.remove(about);
        });
        let text = match expect {
            Expect::Reaction { target, kind, on } => {
                self.mine.update(|m| {
                    m.changed.insert((hex_encode(&target.hash), *kind), *on);
                });
                match (kind, on) {
                    (Reaction::Like, true) => "Liked.",
                    (Reaction::Like, false) => "Unliked.",
                    (Reaction::Recast, true) => "Recast.",
                    (Reaction::Recast, false) => "Recast undone.",
                }
            }
            Expect::Follow { fid, on } => {
                self.mine.update(|m| {
                    m.follow_changed.insert(*fid, *on);
                });
                if *on { "Following." } else { "Unfollowed." }
            }
            Expect::Deleted { target } => {
                self.mine.update(|m| {
                    m.deleted.insert(hex_encode(&target.hash));
                    remember_deleted(&m.deleted);
                });
                "Deleted."
            }
            Expect::Reacted { target, emoji } => {
                self.mine.update(|m| {
                    let reacted = m.emoji.entry(hex_encode(&target.hash)).or_default();
                    if !reacted.contains(emoji) {
                        reacted.push(emoji);
                    }
                    if let Some(reply) = &hash {
                        m.reaction_replies.insert((hex_encode(&target.hash), emoji.code), reply.clone());
                    }
                });
                "Reacted."
            }
            Expect::Posted { cast, settle } => {
                self.mine.update(|m| {
                    if let Some(sent) = m.outgoing.iter_mut().find(|sent| sent.id == cast.id) {
                        sent.hash = hash.clone();
                    }
                });
                if let Some(settle) = settle {
                    settle(true);
                }
                match hash {
                    Some(hash) => return self.posted(&hash),
                    None => "Posted.",
                }
            }
        };
        self.say(text.to_owned());
    }

    /// Whether the reader's like or recast of `hash` shows before it's done
    /// in the Farcaster app.
    pub fn pending_reaction(self, hash: &str, kind: Reaction) -> bool {
        self.mine.with(|m| m.pending.contains(&format!("{kind:?} {hash}")))
    }

    /// Whether the reader's follow of `fid`, or unfollow, shows before it's
    /// done in the Farcaster app.
    pub fn pending_follow(self, fid: u64) -> bool {
        self.mine.with(|m| m.pending.contains(&format!("follow {fid}")))
    }

    /// Whether the reader's reaction to `hash` with `emoji` shows before it's
    /// posted in the Farcaster app.
    pub fn pending_emoji(self, hash: &str, emoji: &Emoji) -> bool {
        self.mine.with(|m| m.pending.contains(&format!("react {hash} {}", emoji.code)))
    }

    /// The casts handed to the Farcaster app this visit, oldest first.
    pub fn outgoing(self) -> Vec<Outgoing> {
        self.mine.with(|m| m.outgoing.clone())
    }

    /// Whether one of the reader's replies to `hash`, handed to the
    /// Farcaster app, isn't posted yet.
    pub fn replying(self, hash: &str) -> bool {
        self.mine.with(|m| {
            m.outgoing
                .iter()
                .any(|c| c.hash.is_none() && matches!(&c.sent, Sent::Reply { parent, .. } if parent == hash))
        })
    }

    /// `hash`'s reply count, `read` as the page has it, with the reader's
    /// replies handed to the Farcaster app since.
    pub fn replies_shown(self, hash: &str, read: u64) -> u64 {
        self.mine.with(|m| {
            let mut mine =
                m.outgoing.iter().filter(|c| matches!(&c.sent, Sent::Reply { parent, .. } if parent == hash));
            match mine.next() {
                Some(first) => read.max(first.replies_before + 1 + mine.count() as u64),
                None => read,
            }
        })
    }

    /// Find out whether `fid` can like and recast here: whether the Worker
    /// endorses keys, and whether this browser keeps an approved one.
    fn check(self, fid: u64) {
        let check = CHECK.with(|c| {
            c.set(c.get() + 1);
            c.get()
        });
        self.can.set(Can::Checking);
        spawn_local(async move {
            let can = match api::get::<Writes>(&writes_path()).await {
                Ok(writes) => {
                    self.app.set(Some(writes.data.app_fid));
                    kept_key(fid).await
                }
                Err(_) => Can::No,
            };
            if CHECK.with(Cell::get) != check {
                return;
            }
            self.can.set(can);
            // What they liked and recasted shows, whichever app they did it
            // in, where Allthing writes for them and where it doesn't.
            if matches!(can, Can::Yes | Can::No) {
                self.load_mine(fid);
            }
            // A write that came before the answer.
            if let Some((then, ask, expect)) = PENDING.with(|p| p.borrow_mut().take()) {
                self.when_ready(then, ask, expect);
            }
        });
    }

    fn signed_out(self, fid: Option<u64>) {
        CHECK.with(|c| c.set(c.get() + 1));
        self.can.set(Can::No);
        KEY.with(|k| k.borrow_mut().take());
        WAITING.with(|w| w.borrow_mut().take());
        PENDING.with(|p| p.borrow_mut().take());
        // What showed of the reader's hand-offs goes with them.
        UNDO.with(|u| u.borrow_mut().clear());
        // What was deleted stays deleted, whoever is signed in.
        self.mine.update(|m| *m = Mine { deleted: std::mem::take(&mut m.deleted), ..Mine::default() });
        if self.open.get_untracked() {
            self.dismiss();
        }
        // Signing out forgets this browser's key: whoever uses it next
        // can't like as the reader.
        if let Some(fid) = fid {
            spawn_local(async move {
                let _ = keys::remove(fid).await;
            });
        }
    }

    /// The reader's latest likes and recasts, from the node.
    fn load_mine(self, fid: u64) {
        for kind in [Reaction::Like, Reaction::Recast] {
            spawn_local(async move {
                let Ok((200, body)) = fetch("GET", &message::reactions_url(fid, kind, RECENT), None).await else {
                    return;
                };
                let hashes = message::reacted(&body).unwrap_or_default();
                if self.fid() == Some(fid) {
                    self.mine.update(|m| m.known.extend(hashes.into_iter().map(|hash| (hash, kind))));
                }
            });
        }
    }

    /// The key was approved: it signs from now on, and the like or recast
    /// that opened the dialog goes out, if the dialog is still open.
    fn approved(self, stored: Stored, run: u64) {
        let fid = stored.fid;
        let waiting = if current(run) { WAITING.with(|w| w.borrow_mut().take()) } else { None };
        KEY.with(|k| *k.borrow_mut() = Some(stored));
        self.can.set(Can::Yes);
        self.load_mine(fid);
        tell_tabs();
        if current(run) {
            self.close();
        }
        if let Some(then) = waiting {
            then();
        }
    }

    /// The node no longer knows the key: the reader removed it in the
    /// Farcaster app. It is forgotten here too.
    fn forget_key(self, fid: u64) {
        KEY.with(|k| k.borrow_mut().take());
        self.can.set(Can::Approve);
        spawn_local(async move {
            let _ = keys::remove(fid).await;
            tell_tabs();
        });
    }

    /// Turn off liking and recasting from this browser: its key is
    /// forgotten.
    fn turn_off(self) {
        let Some(fid) = KEY.with(|k| k.borrow_mut().take().map(|s| s.fid)) else { return };
        self.can.set(Can::Approve);
        self.mine.update(|m| m.known.clear());
        self.say("This browser's key is deleted. To remove it from your account too, remove Allthing in the Farcaster app's settings.".into());
        spawn_local(async move {
            let _ = keys::remove(fid).await;
            tell_tabs();
        });
    }

    fn say(self, message: String) {
        self.notify(Notice { text: message, link: None });
    }

    fn notify(self, notice: Notice) {
        let run = NOTICE_RUN.with(|r| {
            r.set(r.get() + 1);
            r.get()
        });
        self.notice.set(Some(notice));
        spawn_local(async move {
            crate::video::sleep(NOTICE_MS).await;
            if NOTICE_RUN.with(Cell::get) == run {
                self.notice.set(None);
            }
        });
    }

    /// Open the dialog. A key approved in another tab meanwhile is used at
    /// once instead.
    fn connect(self) {
        let Some(fid) = self.fid() else { return };
        RETURN_FOCUS.with(|r| *r.borrow_mut() = document().active_element().and_then(|el| el.dyn_into().ok()));
        spawn_local(async move {
            if let Ok(Some(stored)) = keys::load(fid).await
                && stored.approved_at.is_some()
            {
                let run = RUN.with(Cell::get);
                self.approved(stored, run);
                return;
            }
            self.open.set(true);
            self.begin(fid);
        });
    }

    fn begin(self, fid: u64) {
        let run = RUN.with(|r| {
            r.set(r.get() + 1);
            r.get()
        });
        self.step.set(Step::Starting);
        spawn_local(async move {
            match approve(run, self.step, fid).await {
                Ok(stored) => self.approved(stored, run),
                Err(message) if current(run) && !message.is_empty() => self.step.set(Step::Failed(message)),
                Err(_) => {}
            }
        });
    }

    /// Close the dialog, stopping its polls, and return focus to where it
    /// was. A request the reader hasn't approved yet is kept: its code shows
    /// again next time, until it lapses. The like or recast that opened the
    /// dialog is dropped.
    fn close(self) {
        WAITING.with(|w| w.borrow_mut().take());
        self.dismiss();
        if let Some(element) = RETURN_FOCUS.with(|r| r.borrow_mut().take()).filter(|el| el.is_connected()) {
            let _ = element.focus();
        }
    }

    fn dismiss(self) {
        RUN.with(|r| r.set(r.get() + 1));
        self.open.set(false);
    }

    /// When another tab approves or forgets a key, this one checks again.
    fn follow_tabs(self) {
        let Ok(channel) = web_sys::BroadcastChannel::new(TABS) else { return };
        let heard = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |_: web_sys::MessageEvent| {
            if let Some(fid) = self.fid() {
                KEY.with(|k| k.borrow_mut().take());
                self.check(fid);
            }
        });
        channel.set_onmessage(Some(heard.as_ref().unchecked_ref()));
        heard.forget();
        TABS_CHANNEL.with(|c| *c.borrow_mut() = Some(channel));
    }
}

impl Writer {
    /// Who is signed in.
    fn fid(self) -> Option<u64> {
        self.session.user.with_untracked(|u| u.as_ref().map(|u| u.fid))
    }
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new()
    }
}

fn tell_tabs() {
    TABS_CHANNEL.with(|c| {
        if let Some(channel) = c.borrow().as_ref() {
            let _ = channel.post_message(&JsValue::from_str("changed"));
        }
    });
}

/// The casts deleted earlier in this tab.
fn deleted_before() -> HashSet<String> {
    let stored = web_sys::window()
        .and_then(|w| w.session_storage().ok().flatten())
        .and_then(|s| s.get_item(DELETED).ok().flatten());
    stored.unwrap_or_default().split(',').filter(|hash| CastId::new(1, hash).is_some()).map(str::to_owned).collect()
}

fn remember_deleted(hashes: &HashSet<String>) {
    if let Some(storage) = web_sys::window().and_then(|w| w.session_storage().ok().flatten()) {
        let _ = storage.set_item(DELETED, &hashes.iter().map(String::as_str).collect::<Vec<_>>().join(","));
    }
}

fn current(run: u64) -> bool {
    RUN.with(|r| r.get() == run)
}

fn now_secs() -> f64 {
    js_sys::Date::now() / 1000.0
}

/// What this browser keeps for `fid`: an approved key; or a request the
/// reader approved while no dialog was watching; or nothing to sign with.
async fn kept_key(fid: u64) -> Can {
    let stored = match keys::load(fid).await {
        Ok(Some(stored)) => stored,
        _ => return Can::Approve,
    };
    if stored.approved_at.is_some() {
        KEY.with(|k| *k.borrow_mut() = Some(stored));
        return Can::Yes;
    }
    let Some(request) = stored.request.clone() else { return Can::Approve };
    if request.deadline < now_secs() {
        let _ = keys::remove(fid).await;
        return Can::Approve;
    }
    let answer = fetch("GET", &key_request_url(&request.token), None).await;
    let completed = answer.ok().and_then(|(_, body)| parse_requested(&body)).filter(|requested| {
        requested.state == KeyState::Completed && requested.user_fid == Some(fid) && requested.key == stored.key
    });
    if completed.is_none() {
        return Can::Approve;
    }
    let stored = stored.approved();
    let _ = keys::save(&stored).await;
    KEY.with(|k| *k.borrow_mut() = Some(stored));
    Can::Yes
}

/// The dialog's work: a key and its request (or the one still waiting),
/// the code, then polls until the reader has approved in the Farcaster app,
/// the key is added, and the node knows it.
async fn approve(run: u64, step: RwSignal<Step>, fid: u64) -> Result<Stored, String> {
    let stored = match keys::load(fid).await {
        Ok(Some(stored)) if stored.approved_at.is_some() => return Ok(stored),
        // A request still waiting: the same code, not another key.
        Ok(Some(stored)) if stored.request.as_ref().is_some_and(|r| r.deadline > now_secs() + 120.0) => stored,
        Ok(_) => request(fid).await?,
        Err(error) => return Err(key_message(error)),
    };
    let Some(request) = stored.request.clone() else { return Err(EXPIRED.into()) };
    // A computer shows the link as a code for the reader's phone; a phone
    // opens it in the Farcaster app.
    let code = match can_hover() {
        true => Some(
            api::post::<_, QrCode>(&siwf::qr_path(), &QrRequest { url: request.link.clone() })
                .await
                .map_err(|f| f.message())?,
        ),
        false => None,
    };
    if !current(run) {
        return Err(String::new());
    }
    step.set(Step::Waiting { code, link: request.link });
    loop {
        pause(POLL_MS).await;
        if !current(run) {
            return Err(String::new());
        }
        if now_secs() > request.deadline {
            let _ = keys::remove(fid).await;
            return Err(EXPIRED.into());
        }
        if document().hidden() {
            continue;
        }
        let Ok((status, body)) = fetch("GET", &key_request_url(&request.token), None).await else { continue };
        if matches!(status, 404 | 410) {
            let _ = keys::remove(fid).await;
            return Err(EXPIRED.into());
        }
        let Some(requested) = parse_requested(&body).filter(|r| r.key == stored.key) else { continue };
        if let Some(other) = requested.user_fid.filter(|approver| *approver != fid) {
            let _ = keys::remove(fid).await;
            return Err(format!(
                "That was approved from another Farcaster account (fid {other}). Approve from the account you're signed in with."
            ));
        }
        match requested.state {
            KeyState::Pending => {}
            KeyState::Approved => step.set(Step::Adding),
            KeyState::Completed => break,
        }
    }
    step.set(Step::Adding);
    // The node may take a moment to see the new key.
    for _ in 0..NODE_CHECKS {
        if let Ok((200, body)) = fetch("GET", &message::signers_url(fid), None).await
            && message::lists_key(&body, &stored.key) == Some(true)
        {
            break;
        }
        pause(POLL_MS).await;
        if !current(run) {
            return Err(String::new());
        }
    }
    let stored = stored.approved();
    keys::save(&stored).await.map_err(key_message)?;
    Ok(stored)
}

/// A new key, endorsed by the Worker, and its request in the Farcaster app,
/// kept in this browser.
async fn request(fid: u64) -> Result<Stored, String> {
    let mut stored = keys::generate(fid).await.map_err(key_message)?;
    // An `app_key::KeyRequest`, passed on as it is: only its deadline is
    // read here.
    let endorsed: serde_json::Value =
        api::post(&endorse_path(), &Endorse { key: stored.key.clone() }).await.map_err(|f| f.message())?;
    let deadline = endorsed.get("deadline").and_then(serde_json::Value::as_f64).ok_or(FARCASTER_DOWN)?;
    let body = serde_json::to_string(&endorsed).map_err(|_| FARCASTER_DOWN.to_owned())?;
    let (status, answer) = fetch("POST", &key_requests_url(), Some(("application/json", JsValue::from_str(&body))))
        .await
        .map_err(|_| FARCASTER_DOWN.to_owned())?;
    let requested = parse_requested(&answer).filter(|r| (200..300).contains(&status) && r.key == stored.key);
    let Some(requested) = requested else {
        return Err(match app_key::api_error(&answer) {
            Some(error) => format!("The Farcaster app didn't take the request: {error}"),
            None => FARCASTER_DOWN.to_owned(),
        });
    };
    stored.request = Some(keys::Request { token: requested.token, link: requested.link, deadline });
    keys::save(&stored).await.map_err(key_message)?;
    Ok(stored)
}

fn key_message(error: KeyError) -> String {
    match error {
        KeyError::Unsupported => UNSUPPORTED.to_owned(),
        KeyError::Failed(message) => message,
    }
}

/// Why a like or a recast didn't go out.
enum Unsent {
    KeyGone,
    Failed(String),
}

/// Who `names` (mentioned as "@name") belong to, as far as the node knows:
/// asked one by one, and kept for the visit.
pub async fn fids_of(names: &[String]) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for name in names {
        let known = NAMES.with(|n| n.borrow().get(name).copied());
        let fid = match known {
            Some(fid) => fid,
            None => match fetch("GET", &message::username_url(name), None).await {
                Ok((status, body)) => match message::username_fid(status, &body) {
                    Some(fid) => {
                        NAMES.with(|n| n.borrow_mut().insert(name.clone(), fid));
                        fid
                    }
                    None => None,
                },
                Err(()) => None,
            },
        };
        if let Some(fid) = fid {
            out.insert(name.clone(), fid);
        }
    }
    out
}

/// Sign `body` with the reader's key and hand it to the node: the
/// message's hash. A like, a recast or a follow (`slot`) that undoes
/// another goes out at least a second after it, so it wins.
async fn send(stored: &Stored, body: Body, slot: Option<&String>) -> Result<[u8; 20], Unsent> {
    let signer = app_key::parse_public_key(&stored.key).ok_or(Unsent::KeyGone)?;
    let fresh = stored.approved_at.is_some_and(|at| js_sys::Date::now() - at < FRESH_MS);
    let mut wait = RETRY_MS;
    for tried in 0..=RETRIES {
        let now = now_secs() as i64;
        let at = slot.map_or(now, |key| {
            SENT_AT.with(|s| {
                let mut sent = s.borrow_mut();
                let at = now.max(sent.get(key).map_or(0, |last| last + 1));
                sent.insert(key.clone(), at);
                at
            })
        });
        let data = MessageData::new(stored.fid, at, body.clone()).encode();
        let hash = message::message_hash(&data);
        let signature = keys::sign(stored, &hash).await.map_err(|e| Unsent::Failed(key_message(e)))?;
        let envelope = message::envelope(&data, &hash, &signature, &signer);
        let bytes = js_sys::Uint8Array::from(envelope.as_slice());
        let (status, answer) = fetch("POST", &message::submit_url(), Some(("application/octet-stream", bytes.into())))
            .await
            .map_err(|_| Unsent::Failed(NODE_DOWN.into()))?;
        match message::submitted(status, &answer) {
            Submitted::Merged | Submitted::Duplicate => return Ok(hash),
            // A key approved a moment ago may not have reached this node.
            Submitted::UnknownKey if fresh && tried < RETRIES => {
                crate::video::sleep(wait).await;
                wait *= 2;
            }
            Submitted::UnknownKey => return Err(Unsent::KeyGone),
            Submitted::Unavailable => return Err(Unsent::Failed(NODE_DOWN.into())),
            Submitted::Refused(why) if why.is_empty() => return Err(Unsent::Failed(NODE_DOWN.into())),
            Submitted::Refused(why) => return Err(Unsent::Failed(format!("Farcaster didn't take it: {why}"))),
        }
    }
    Err(Unsent::KeyGone)
}

/// A request to the node or the Farcaster app's API: its status and body.
async fn fetch(method: &str, url: &str, body: Option<(&str, JsValue)>) -> Result<(u16, String), ()> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    if let Some((content_type, body)) = body {
        let headers = web_sys::Headers::new().map_err(|_| ())?;
        headers.set("content-type", content_type).map_err(|_| ())?;
        init.set_headers(&headers);
        init.set_body(&body);
    }
    let window = web_sys::window().ok_or(())?;
    let abort = web_sys::AbortController::new().ok();
    if let Some(abort) = &abort {
        init.set_signal(Some(&abort.signal()));
        let stop = abort.clone();
        let timer = Closure::once_into_js(move || stop.abort());
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(timer.unchecked_ref(), TIMEOUT_MS);
    }
    let response = JsFuture::from(window.fetch_with_str_and_init(url, &init)).await.map_err(|_| ())?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| ())?;
    let text = JsFuture::from(response.text().map_err(|_| ())?).await.map_err(|_| ())?;
    Ok((response.status(), text.as_string().unwrap_or_default()))
}

/// Wait `ms`, or less if the page comes back into view meanwhile.
async fn pause(ms: i32) {
    let wakes = WAKES.with(Cell::get);
    let mut waited = 0;
    while waited < ms {
        if WAKES.with(Cell::get) != wakes {
            return;
        }
        crate::video::sleep(250).await;
        waited += 250;
    }
}

/// The dialog that shows the code to approve Allthing's key with, and the
/// notice for likes and recasts that didn't go out.
#[component]
pub fn WriterDialog() -> impl IntoView {
    let Some(writer) = use_writer() else { return ().into_any() };
    let session = writer.session;
    let dialog = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let open = writer.open.get();
        if let Some(root) = document().document_element() {
            let _ = root.class_list().toggle_with_force("key-open", open);
        }
    });
    let keys = window_event_listener(ev::keydown, move |ev| {
        if !writer.open.get_untracked() {
            return;
        }
        match ev.key().as_str() {
            "Escape" => {
                ev.prevent_default();
                writer.close();
            }
            "Tab" => {
                if let Some(root) = dialog.get_untracked() {
                    trap_tab(&root, &ev, "button, a[href]");
                }
            }
            _ => {}
        }
    });
    // Back in view (the phone's own app, another tab): ask at once.
    let woken = |_| {
        if !document().hidden() {
            WAKES.with(|w| w.set(w.get() + 1));
        }
    };
    let shown = window_event_listener_untyped("visibilitychange", woken);
    let focused = window_event_listener_untyped("focus", woken);
    on_cleanup(move || {
        keys.remove();
        shown.remove();
        focused.remove();
    });
    let status = move || match writer.step.get() {
        Step::Starting => "Getting a code…",
        Step::Waiting { .. } => "Waiting for you to approve in Farcaster…",
        Step::Adding => "Approved. Adding Allthing's key to your account…",
        Step::Failed(_) => "",
    };
    let failed = move || match writer.step.get() {
        Step::Failed(message) => Some(message),
        _ => None,
    };
    let dialog_view = move || {
        writer.open.get().then(|| {
            let close_button = NodeRef::<leptos::html::Button>::new();
            Effect::new(move |_| {
                if let Some(button) = close_button.get() {
                    let _ = button.focus();
                }
            });
            view! {
                <div class="modal-backdrop" on:click=move |ev: ev::MouseEvent| {
                    if ev.target() == ev.current_target() {
                        writer.close();
                    }
                }>
                    <div class="modal signin-modal key-modal" role="dialog" aria-modal="true" aria-labelledby="key-title" node_ref=dialog>
                        <header class="modal-head">
                            <h2 id="key-title">"Approve Allthing"</h2>
                            <button type="button" class="modal-close" aria-label="Close" node_ref=close_button on:click=move |_| writer.close()>"×"</button>
                        </header>
                        <div class="modal-body signin-body">
                            <p class="signin-intro">
                                {if can_hover() {
                                    "Once, to post, like, recast and follow from here: scan the code with your phone's camera, then approve Allthing in the Farcaster app."
                                } else {
                                    "Once, to post, like, recast and follow from here: approve Allthing in the Farcaster app."
                                }}
                            </p>
                            <div
                                class="qr-frame"
                                class:failed=move || failed().is_some()
                                class:as-link=move || matches!(writer.step.get(), Step::Waiting { code: None, .. })
                            >
                                {move || match writer.step.get() {
                                    Step::Waiting { code: Some(code), .. } => view! {
                                        <svg class="qr" viewBox=format!("0 0 {0} {0}", code.size) role="img" aria-label="QR code to approve Allthing" shape-rendering="crispEdges">
                                            <path d=code.path/>
                                        </svg>
                                    }
                                    .into_any(),
                                    Step::Waiting { code: None, link } => view! {
                                        <a class="button key-approve" href=link>"Approve in Farcaster"</a>
                                    }
                                    .into_any(),
                                    // The reader, while their key is added.
                                    Step::Adding => view! {
                                        <span class="qr-approver" aria-hidden="true">
                                            <Avatar url=session.user.with_untracked(|u| u.as_ref().and_then(|u| u.pfp_url.clone())) size=88 large=true/>
                                            <span class="qr-approver-check">"✓"</span>
                                        </span>
                                    }
                                    .into_any(),
                                    Step::Failed(_) => view! {
                                        <button type="button" class="button" on:click=move |_| {
                                            if let Some(fid) = writer.fid() {
                                                writer.begin(fid);
                                            }
                                        }>"Get a new code"</button>
                                    }
                                    .into_any(),
                                    Step::Starting => view! { <span class="qr-loading" aria-hidden="true"></span> }.into_any(),
                                }}
                            </div>
                            <p class="signin-status" class:error=move || failed().is_some() role="status">
                                {move || failed().unwrap_or_else(|| status().to_owned())}
                            </p>
                            <p class="signin-note muted small">
                                // The Farcaster app names the account that asks: Allthing's
                                // own, so a reader can check it is.
                                {move || {
                                    (writer.app.get() == Some(ALLTHING_APP_FID))
                                        .then_some("The Farcaster app shows the request as from Allthing, @allthingapp. ")
                                }}
                                "Allthing gets a key that can like, recast and post for you, and uses it only when you press a button. It stays in this browser, and Allthing's servers never see it. Signing out deletes it."
                            </p>
                        </div>
                    </div>
                </div>
            }
        })
    };
    view! {
        {dialog_view}
        <p class="toast" role="status" aria-live="polite" class:shown=move || writer.notice.with(Option::is_some)>
            {move || writer.notice.get().map(|notice| view! {
                {notice.text}
                {notice.link.map(|(href, label)| view! { " "<a href=href>{label}</a> })}
            })}
        </p>
    }
    .into_any()
}

/// The account menu's link to the reader's scheduled casts, wherever they
/// can write from here.
#[component]
pub fn ScheduledMenuItem() -> impl IntoView {
    let Some(writer) = use_writer() else { return ().into_any() };
    (move || writer.can_write().then(|| view! { <a class="account-link" href=scheduled_href()>"Scheduled casts"</a> }))
        .into_any()
}

/// The account menu's line about likes and recasts: on in this browser,
/// with a way to turn it off.
#[component]
pub fn WriterMenuItem() -> impl IntoView {
    let Some(writer) = use_writer() else { return ().into_any() };
    (move || {
        (writer.can.get() == Can::Yes).then(|| {
            view! {
                <button type="button" class="account-link turn-off" on:click=move |_| writer.turn_off()>
                    "Stop liking from this browser"
                </button>
            }
        })
    })
    .into_any()
}

/// Whether the reader deleted `hash` this visit, for a view to show so.
pub fn deleted_memo(hash: String) -> Memo<bool> {
    let writer = use_writer();
    Memo::new(move |_| writer.is_some_and(|w| w.deleted(&hash)))
}

/// Follow, or Following, on a profile: on anyone's profile but the
/// reader's own. Signed in, it shows once the node has said whether they
/// follow already (a read that needs no key); hovering Following offers
/// Unfollow. Where Allthing doesn't write for the reader, it opens the
/// profile in the Farcaster app instead (`components::handoff`).
#[component]
pub fn FollowButton(fid: u64, username: String) -> impl IntoView {
    let Some(writer) = use_writer() else { return ().into_any() };
    let shown = Memo::new(move |_| writer.reader() != Some(fid));
    let known = Memo::new(move |_| writer.follows(fid));
    // Asked once for each reader signed in.
    Effect::new(move |looked: Option<Option<u64>>| {
        let me = writer.reader().filter(|_| shown.get());
        if me.is_some() && looked.flatten() != me {
            writer.look_up_follow(fid);
        }
        me.or(looked.flatten())
    });
    let label = format!("Follow @{username}");
    let url = profile_url(&username, fid);
    (move || {
        shown.get().then(|| {
            let following = move || known.get().unwrap_or(false);
            let (url, username) = (url.clone(), username.clone());
            view! {
                <button
                    type="button"
                    class="follow-button"
                    class:following=following
                    class:pending=move || writer.pending_follow(fid)
                    class:unknown=move || writer.reader().is_some() && known.get().is_none()
                    aria-label=label.clone()
                    aria-pressed=move || writer.reader().is_some().then(|| following().to_string())
                    aria-haspopup=move || writer.hands_off().then_some("dialog")
                    aria-busy=move || writer.follow_busy(fid).then_some("true")
                    on:click=move |_| writer.toggle_follow(fid, Ask::follow(url.clone(), &username, following()))
                >
                    <span class="follow-text">{move || if following() { "Following" } else { "Follow" }}</span>
                    <span class="unfollow-text" aria-hidden="true">"Unfollow"</span>
                </button>
            }
        })
    })
    .into_any()
}

/// A profile's follower count, with the reader's follow or unfollow here.
#[component]
pub fn FollowerCount(fid: u64, n: u64) -> impl IntoView {
    let writer = use_writer();
    let shown = Memo::new(move |_| (n as i64 + writer.map_or(0, |w| w.followers_delta(fid))).max(0) as u64);
    move || view! { <crate::components::chips::Count n=shown.get() label="followers"/> }
}

/// Delete, under one of the reader's own casts: it asks once more, in
/// place, with Keep focused, before it deletes. Where Allthing doesn't write
/// for the reader, it opens the cast in the Farcaster app to delete there.
#[component]
pub fn DeleteButton(hash: String, author: u64, username: String, noun: &'static str) -> impl IntoView {
    let Some(writer) = use_writer() else { return ().into_any() };
    let gone = deleted_memo(hash.clone());
    let own = Memo::new(move |_| writer.reader() == Some(author) && !gone.get());
    let url = cast_url(&username, &hash);
    let key = hash.clone();
    let busy = Memo::new(move |_| writer.deleting(&key));
    let asking = RwSignal::new(false);
    let question = format!("Delete this {noun}?");
    (move || {
        own.get().then(|| {
            let hash = hash.clone();
            if asking.get() {
                let keep = NodeRef::<leptos::html::Button>::new();
                Effect::new(move |_| {
                    if let Some(button) = keep.get() {
                        let _ = button.focus();
                    }
                });
                view! {
                    <span class="delete-confirm" role="group" aria-label=question.clone()>
                        <span class="delete-question">{question.clone()}</span>
                        <button type="button" class="delete-yes" on:click={
                            let url = url.clone();
                            move |_| {
                                asking.set(false);
                                writer.delete(hash.clone(), Ask::delete(url.clone(), noun));
                            }
                        }>"Delete"</button>
                        <button type="button" class="delete-no" node_ref=keep on:click=move |_| asking.set(false)>"Keep"</button>
                    </span>
                }
                .into_any()
            } else {
                // Nothing is deleted here when it goes to the app: no second
                // ask, and it shows as deleted once the reader has, there.
                let url = url.clone();
                view! {
                    <button
                        type="button"
                        class="reply-button delete-button"
                        disabled=busy
                        aria-busy=move || busy.get().then_some("true")
                        aria-haspopup=move || writer.hands_off().then_some("dialog")
                        on:click=move |_| {
                            if writer.hands_off() {
                                writer.delete(hash.clone(), Ask::delete(url.clone(), noun));
                            } else {
                                asking.set(true);
                            }
                        }
                    >
                        <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=TRASH/></svg>
                        {move || if busy.get() { "Deleting…" } else { "Delete" }}
                    </button>
                }
                .into_any()
            }
        })
    })
    .into_any()
}

const TRASH: &str = "M3.5 4.5h9M6.5 4.5v-1.5h3v1.5M5 4.5l.5 8.5h5l.5-8.5";

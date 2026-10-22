// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writing a cast, a reply or a quote, with a GIF or emoji, for any reader.
//! Cast, at the top right of the front page's feed or a channel's (a round
//! button over the feed on a phone), starts a cast, in the channel on a
//! channel page, and so does the same pill on the reader's own profile;
//! Reply and Quote under a post or a reply start a reply to it or a cast
//! that quotes it.
//!
//! Where the reader can like and recast here (`components::writer`), the
//! draft is posted with their key: "@name"s become mentions, and the cast
//! goes to the node from their browser (`allthing_core::compose::Draft::
//! new_cast`). Signed out, or where the Worker endorses no keys (as on
//! allthing.social), "Post in Farcaster" opens the draft in the Farcaster
//! app instead, through farcaster.xyz's compose link, for the reader to
//! post there: at once on a phone, and on a computer through the hand-off
//! dialog, with the link as a QR code for their phone (`components::handoff`).
//!
//! GIFs come from KLIPY. Its terms ask that the browser reach its API and
//! its media itself, never through Allthing's server, and that results show
//! in the order KLIPY gives them, in a grid of their own, under a search box
//! that says "Search KLIPY". The key comes from the Worker
//! (`/api/v1/gifs/key`, for a signed-in session); without one (no
//! `KLIPY_API_KEY` secret) the composer has no GIF button. Emoji and
//! stickers come from Allthing's own pack (`components::emoji_picker`); a
//! sticker takes the GIF's place.
//!
//! Images and videos go with it too, from the attach button (a phone's
//! photo picker), a paste or a drop (`components::attach`), once the Worker
//! has said it takes them (`/api/v1/media`, for a reader signed in). Each is
//! uploaded as it's added, and the cast embeds its URL. A cast carries two
//! embeds: its files, its GIF or sticker, and the cast it quotes, together.
//! Where the Worker takes none (no R2 bucket bound), there's no trace of
//! them: no button, and pastes and drops go as they did.
//!
//! The clock schedules the draft for later: it is signed with the reader's
//! key at once, for the time they pick, and kept by the Worker until then
//! (`allthing_core::scheduled`). A scheduled cast opens here again to be
//! changed (`Composer::edit`). There is no clock where the draft goes to the
//! Farcaster app. Not in the static copy, which has no sign-in.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use allthing_core::api::{ChannelPayload, channel_path};
use allthing_core::channels::is_channel_id;
use allthing_core::compose::{
    Draft, Gif, GifKey, GifPage, Quoted, Rendition, SEARCH_PLACEHOLDER, gif_key_path, gifs_url,
};
use allthing_core::emoji::{self, STICKER_HEIGHT, Sticker};
use allthing_core::media::{EMBEDS_FULL, MEDIA_PATH, MediaLimits, MediaType, embeds_left};
use allthing_core::message::{MAX_CAST_BYTES, Sent};
use allthing_core::model::{Cast, UserSummary};
use allthing_core::params::{fid_username, username_from_param};
use allthing_core::scheduled::{MAX_AHEAD, MIN_AHEAD, SavedCast, SavedDraft, SavedGif, SavedMedia, ScheduledCast};
use allthing_core::text::mention_names;
use allthing_core::text::truncate_title;
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::wasm_split::{SplitLoaderError, wasm_split};
use leptos_router::hooks::{use_location, use_query_map};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlElement;

use crate::api;
use crate::components::attach::{self, AttachButton, Attached};
use crate::components::editor::{self, ComposeText};
use crate::components::emoji_picker::{self, EmojiPicker, Pick, Tab};
use crate::components::handoff::{Ask, use_hand_off};
use crate::components::signin::{self, use_session};
use crate::components::text::ExcerptText;
use crate::components::writer::{Expect, Outgoing, Settle, Writer, fids_of, use_writer};
use crate::util::{
    BASE, can_hover, document, from_local_input, href, local_input, now, scheduled_href, time_zone, trap_tab,
};

/// How long typing pauses before a search is sent.
const DEBOUNCE: Duration = Duration::from_millis(350);
/// How long KLIPY's answers are reused this visit.
const KEEP_MS: f64 = 600_000.0;
/// The shortest search sent (an empty one shows what's trending).
const MIN_QUERY: usize = 2;

const PEN: &str = "M10.5 3.2l2.3 2.3M3 13l.6-2.9 7.5-7.5a1.2 1.2 0 0 1 1.7 0l.6.6a1.2 1.2 0 0 1 0 1.7l-7.5 7.5z";
const REPLY: &str = "M6.5 4L3 7.5 6.5 11M3.5 7.5h6a3.5 3.5 0 0 1 3.5 3.5v1";
const OUT: &str = "M9 3h4v4M13 3L7.5 8.5M11.5 9.5v3h-8v-8h3";
const QUOTE: &str = "M3.5 4.5h9v6h-5l-2.5 2v-2h-1.5zM6 7.5h4";
/// A sticker with its corner peeling.
const STICKER: &str = "M5.5 2h5A3.5 3.5 0 0 1 14 5.5V9l-5 5H5.5A3.5 3.5 0 0 1 2 10.5v-5A3.5 3.5 0 0 1 5.5 2zM14 9h-2.5A2.5 2.5 0 0 0 9 11.5V14M5.8 8.6c.6.7 1.4 1 2.2 1";
/// The text's length starts showing at this many bytes.
const COUNT_FROM: usize = 280;
/// The clock's hands, inside its face: the button that schedules for later.
const CLOCK: &str = "M8 4.6V8l2.4 1.6";

/// A page of KLIPY's GIFs, and whether there are more.
type Found = Rc<(Vec<Gif>, bool)>;

thread_local! {
    /// KLIPY's pages this visit, by request, and when they came: reopening
    /// the picker, or going back to an earlier search, asks for nothing again.
    static PAGES: RefCell<HashMap<String, (f64, Found)>> = RefCell::new(HashMap::new());
    static RETURN_FOCUS: RefCell<Option<HtmlElement>> = const { RefCell::new(None) };
    /// What each attachment's upload needs of the browser, by its id.
    pub(crate) static LIVE: RefCell<HashMap<u64, Live>> = RefCell::new(HashMap::new());
    /// Files pasted or dropped, for the composer's part to take
    /// (`attach::take_incoming`).
    pub(crate) static INCOMING: RefCell<Vec<web_sys::File>> = const { RefCell::new(Vec::new()) };
    static NEXT_FILE: Cell<u64> = const { Cell::new(1) };
}

/// An image or a video the draft carries, and how its upload goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub id: u64,
    /// The file's name, for its labels and what is said of it.
    pub name: String,
    pub media: MediaType,
    /// The file in this browser (`URL.createObjectURL`), shown while the
    /// draft lasts; none for one taken up again, which shows from its URL.
    pub local: Option<String>,
    pub state: Sending,
}

/// How an attachment's upload goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Sending {
    /// On its way: bytes sent, of how many.
    Up { sent: f64, of: f64 },
    /// Kept by the Worker, at the URL the cast embeds.
    Done { url: String },
    /// Refused, or lost on the way: why, as the Worker or the browser says.
    Failed { why: String },
}

impl Attachment {
    /// A new attachment's id.
    pub fn next_id() -> u64 {
        NEXT_FILE.with(|n| {
            let id = n.get();
            n.set(id + 1);
            id
        })
    }

    /// The URL the cast embeds, once it's uploaded.
    pub fn url(&self) -> Option<&str> {
        match &self.state {
            Sending::Done { url } => Some(url),
            _ => None,
        }
    }

    /// Where its preview shows from.
    pub fn preview(&self) -> String {
        self.local.clone().or_else(|| self.url().map(str::to_owned)).unwrap_or_default()
    }

    /// One a draft kept, taken up again: uploaded, shown from its URL.
    fn kept(saved: &SavedMedia) -> Option<Attachment> {
        let name = saved.url.rsplit('/').next().unwrap_or_default().to_owned();
        let media = MediaType::of_file(&name, &saved.media_type)?;
        Some(Attachment {
            id: Attachment::next_id(),
            name,
            media,
            local: None,
            state: Sending::Done { url: saved.url.clone() },
        })
    }

    /// The same, without its preview in this browser, which goes with the
    /// draft: for a draft kept to come back.
    fn kept_copy(&self) -> Attachment {
        Attachment { local: None, ..self.clone() }
    }
}

/// What an attachment's upload needs of the browser, which no signal holds:
/// the file, what is sent (the file, or the file without its metadata), the
/// request on its way, and its handlers.
pub struct Live {
    pub file: web_sys::File,
    pub body: Option<web_sys::Blob>,
    /// A video remuxed for HLS: its index (`allthing_core::fmp4`), sent with
    /// `body`, which is then the remuxed MP4.
    pub hls: Option<String>,
    pub request: Option<web_sys::XmlHttpRequest>,
    pub progress: Option<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::ProgressEvent)>>,
    pub ended: Option<wasm_bindgen::closure::Closure<dyn FnMut()>>,
}

/// Let an attachment go: its upload stopped if it's on its way, and its
/// preview's object URL let go.
pub fn let_go(file: &Attachment) {
    if let Some(live) = LIVE.with(|l| l.borrow_mut().remove(&file.id))
        && let Some(request) = &live.request
    {
        request.set_onload(None);
        request.set_onerror(None);
        if let Ok(upload) = request.upload() {
            upload.set_onprogress(None);
        }
        let _ = request.abort();
    }
    if let Some(local) = &file.local {
        let _ = web_sys::Url::revoke_object_url(local);
    }
}

/// The cast a reply answers or a quote quotes.
#[derive(Clone, Debug, PartialEq)]
struct Other {
    hash: String,
    /// Its author.
    fid: u64,
    username: String,
    excerpt: String,
}

impl Other {
    fn of(cast: &Cast) -> Other {
        Other {
            hash: cast.hash.clone(),
            fid: cast.author.fid,
            username: cast.author.username.clone(),
            excerpt: truncate_title(cast.text.trim(), 90),
        }
    }

    fn saved(&self) -> SavedCast {
        SavedCast {
            hash: self.hash.clone(),
            fid: self.fid,
            username: self.username.clone(),
            excerpt: self.excerpt.clone(),
        }
    }

    fn from_saved(saved: &SavedCast) -> Other {
        Other {
            hash: saved.hash.clone(),
            fid: saved.fid,
            username: saved.username.clone(),
            excerpt: saved.excerpt.clone(),
        }
    }
}

/// The picker open under the draft, if one is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Picker {
    Closed,
    Gifs,
    Emoji,
}

impl Picker {
    /// The button that opens it on `tab`, for focus to go back to.
    fn button(self, tab: Tab) -> Option<&'static str> {
        match (self, tab) {
            (Picker::Closed, _) => None,
            (Picker::Gifs, _) => Some(".gif-button"),
            (Picker::Emoji, Tab::Emoji) => Some(".emoji-button"),
            (Picker::Emoji, Tab::Stickers) => Some(".sticker-button"),
        }
    }
}

/// What a draft is for.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// A cast, in a channel or not.
    Cast(Option<String>),
    Reply(Other),
    Quote(Other),
}

/// The composer, held by the app: whether it is open, what the draft is
/// for, and the draft, kept while it is closed unposted.
#[derive(Clone, Copy)]
pub struct Composer {
    open: RwSignal<bool>,
    target: RwSignal<Target>,
    text: RwSignal<String>,
    gif: RwSignal<Option<Gif>>,
    picker: RwSignal<Picker>,
    /// The emoji picker's tab, emoji or stickers.
    tab: RwSignal<Tab>,
    /// Being posted with the reader's key, and why it didn't go out.
    posting: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    /// When it goes out, as the date-and-time field holds it, while it is
    /// scheduled for later.
    when: RwSignal<Option<String>>,
    /// The scheduled cast it changes, when it is one.
    editing: RwSignal<Option<i64>>,
    /// The channel URL of a scheduled cast being changed, whose channel page
    /// the browser may not hold.
    channel_url: RwSignal<Option<String>>,
    /// For a reply, its cast's reply count as the page read it.
    replies: RwSignal<u64>,
    /// The images and videos it carries, in order.
    pub files: RwSignal<Vec<Attachment>>,
    /// This server's limits for them, once it has said it takes them
    /// (`/api/v1/media` answered 200). Until then, and where it doesn't (no
    /// bucket, or the reader signed out), the composer neither shows nor
    /// takes files.
    pub limits: RwSignal<Option<MediaLimits>>,
    /// A word about what was just asked: a file refused, no room for more.
    pub note: RwSignal<Option<String>>,
    /// Moves on as files are pasted or dropped (`INCOMING`).
    pub incoming: RwSignal<u32>,
    /// Files are being dragged over it.
    dragging: RwSignal<bool>,
}

impl Composer {
    pub fn new() -> Composer {
        Composer {
            open: RwSignal::new(false),
            target: RwSignal::new(Target::Cast(None)),
            text: RwSignal::new(String::new()),
            gif: RwSignal::new(None),
            picker: RwSignal::new(Picker::Closed),
            tab: RwSignal::new(Tab::Emoji),
            posting: RwSignal::new(false),
            error: RwSignal::new(None),
            when: RwSignal::new(None),
            editing: RwSignal::new(None),
            channel_url: RwSignal::new(None),
            replies: RwSignal::new(0),
            files: RwSignal::new(Vec::new()),
            limits: RwSignal::new(None),
            note: RwSignal::new(None),
            incoming: RwSignal::new(0),
            dragging: RwSignal::new(false),
        }
    }

    /// Open the composer for `target`. A draft for something else is set
    /// aside; one for the same is taken up again.
    fn start(self, target: Target) {
        RETURN_FOCUS.with(|r| *r.borrow_mut() = document().active_element().and_then(|el| el.dyn_into().ok()));
        // A scheduled cast left unsaved isn't taken up as a new one.
        if self.target.get_untracked() != target || self.editing.get_untracked().is_some() {
            self.target.set(target);
            self.clear();
        }
        self.picker.set(Picker::Closed);
        self.error.set(None);
        self.open.set(true);
    }

    fn clear(self) {
        self.text.set(String::new());
        self.gif.set(None);
        self.error.set(None);
        self.when.set(None);
        self.editing.set(None);
        self.channel_url.set(None);
        self.files.get_untracked().iter().for_each(let_go);
        self.files.set(Vec::new());
        self.note.set(None);
    }

    /// Open one of the reader's scheduled casts, to change it or its time:
    /// what goes out instead is signed again, in its place. A cast whose time
    /// has passed (one that didn't go out) is given a new one.
    pub fn edit(self, cast: &ScheduledCast) {
        RETURN_FOCUS.with(|r| *r.borrow_mut() = document().active_element().and_then(|el| el.dyn_into().ok()));
        self.clear();
        let saved = SavedDraft::parse(&cast.draft);
        let target = match (&saved.reply, &saved.quote) {
            (Some(to), _) => Target::Reply(Other::from_saved(to)),
            (None, Some(of)) => Target::Quote(Other::from_saved(of)),
            (None, None) => Target::Cast(saved.channel.clone()),
        };
        self.target.set(target);
        self.text.set(saved.text.clone());
        self.gif.set(saved.gif.as_ref().map(|gif| Gif {
            slug: String::new(),
            title: gif.title.clone(),
            tile: Rendition { url: gif.tile.clone(), width: gif.width, height: gif.height },
            embed: Rendition { url: gif.embed.clone(), width: gif.width, height: gif.height },
        }));
        self.files.set(saved.media.iter().filter_map(Attachment::kept).collect());
        self.channel_url.set(saved.channel_url.clone());
        let at = if cast.at >= now() + MIN_AHEAD { cast.at } else { later(now()) };
        self.when.set(Some(local_input(at)));
        self.editing.set(Some(cast.id));
        self.picker.set(Picker::Closed);
        self.error.set(None);
        self.open.set(true);
    }

    /// Start a cast to schedule, from the reader's list of scheduled casts:
    /// the composer with its clock on, an hour or so from now to begin
    /// with (or the time the draft had).
    pub fn start_scheduled(self) {
        self.start(Target::Cast(None));
        if self.when.get_untracked().is_none() {
            self.when.set(Some(local_input(later(now()))));
        }
    }

    /// Schedule the draft for later, an hour or so from now to begin with,
    /// or post it now after all.
    fn toggle_when(self) {
        let when = if self.when.get_untracked().is_some() { None } else { Some(local_input(later(now()))) };
        self.when.set(when);
        self.error.set(None);
    }

    /// The draft as its scheduled cast keeps it, for the reader's list and
    /// for changing it later.
    fn saved(self, draft: &Draft) -> SavedDraft {
        let gif = self.gif.with_untracked(|gif| {
            gif.as_ref().map(|g| SavedGif {
                title: g.title.clone(),
                embed: g.embed.url.clone(),
                tile: g.tile.url.clone(),
                width: g.tile.width,
                height: g.tile.height,
            })
        });
        let media = self.files.with_untracked(|files| {
            files
                .iter()
                .filter_map(|f| Some(SavedMedia { url: f.url()?.to_owned(), media_type: f.media.mime().to_owned() }))
                .collect()
        });
        let mut saved = SavedDraft {
            text: self.text.get_untracked(),
            gif,
            media,
            channel: draft.channel.clone(),
            channel_url: draft.channel_url.clone(),
            ..SavedDraft::default()
        };
        match self.target.get_untracked() {
            Target::Reply(to) => saved.reply = Some(to.saved()),
            Target::Quote(of) => saved.quote = Some(of.saved()),
            Target::Cast(_) => {}
        }
        saved
    }

    /// Close, keeping the draft, and return focus to where it was. Files
    /// dropped that weren't taken yet are let go.
    fn close(self) {
        self.open.set(false);
        self.picker.set(Picker::Closed);
        self.dragging.set(false);
        INCOMING.with(|incoming| incoming.borrow_mut().clear());
        if let Some(element) = RETURN_FOCUS.with(|r| r.borrow_mut().take()).filter(|el| el.is_connected()) {
            let _ = element.focus();
        }
    }

    /// Open `picker`, or close it if it is open.
    fn toggle(self, picker: Picker) {
        self.picker.update(|open| *open = if *open == picker { Picker::Closed } else { picker });
    }

    /// Whether the emoji picker is open on `tab`.
    fn showing(self, tab: Tab) -> bool {
        self.picker.get() == Picker::Emoji && self.tab.get() == tab
    }

    /// Open the emoji picker on `tab`, or close it if it shows that already.
    fn toggle_tab(self, tab: Tab) {
        if self.picker.get_untracked() == Picker::Emoji && self.tab.get_untracked() == tab {
            self.picker.set(Picker::Closed);
        } else {
            self.tab.set(tab);
            self.picker.set(Picker::Emoji);
        }
    }

    /// Whether the draft quotes a cast, which is one of its embeds.
    fn quoting(self) -> bool {
        matches!(self.target.get(), Target::Quote(_))
    }

    /// How many more embeds the draft takes.
    pub fn embeds_left(self) -> usize {
        embeds_left(self.files.with(Vec::len), self.gif.with(Option::is_some), self.quoting())
    }

    /// Whether a GIF or a sticker can go with it: in the place of the one
    /// it has, or in a free place.
    pub fn room_for_gif(self) -> bool {
        self.gif.with(Option::is_some) || self.embeds_left() > 0
    }

    /// Whether a file is on its way.
    pub fn uploading(self) -> bool {
        self.files.with(|files| files.iter().any(|f| matches!(f.state, Sending::Up { .. })))
    }

    /// Whether a file didn't go.
    pub fn upload_failed(self) -> bool {
        self.files.with(|files| files.iter().any(|f| matches!(f.state, Sending::Failed { .. })))
    }

    /// Take a file off the draft, stopping its upload.
    pub fn remove_file(self, id: u64) {
        let gone = self.files.with_untracked(|files| files.iter().find(|f| f.id == id).cloned());
        if let Some(file) = gone {
            let_go(&file);
            self.files.update(|files| files.retain(|f| f.id != id));
            self.note.set(None);
        }
    }

    /// Whether this server takes files with a cast, as it has said.
    fn takes_files(self) -> bool {
        self.limits.with_untracked(Option::is_some)
    }

    /// Files pasted (`dropped` false) or dropped on the composer, where this
    /// server takes them: taken for the composer's part to attach
    /// (`attach::take_incoming`); whether they were. Text copied from a word
    /// processor, a spreadsheet or a page comes with a picture of itself,
    /// which isn't what's meant: it's pasted as text.
    fn take_files(self, data: &web_sys::DataTransfer, dropped: bool) -> bool {
        if !self.takes_files() {
            return false;
        }
        let Some(list) = data.files().filter(|list| list.length() > 0) else { return false };
        let has = |kind: &str| data.types().includes(&wasm_bindgen::JsValue::from_str(kind), 0);
        let text = data.get_data("text/plain").is_ok_and(|text| !text.trim().is_empty());
        if !dropped && text && (has("text/html") || has("text/rtf")) {
            return false;
        }
        INCOMING.with(|incoming| incoming.borrow_mut().extend((0..list.length()).filter_map(|i| list.get(i))));
        self.incoming.update(|n| *n = n.wrapping_add(1));
        true
    }

    fn draft(self) -> Draft {
        let mut embeds: Vec<String> =
            self.files.with(|files| files.iter().filter_map(|f| f.url().map(str::to_owned)).collect());
        self.gif.with(|gif| embeds.extend(gif.iter().map(|g| g.embed.url.clone())));
        let mut draft = Draft { text: self.text.get(), embeds, ..Draft::default() };
        match self.target.get() {
            Target::Reply(to) => {
                draft.parent_hash = Some(to.hash);
                draft.parent_fid = Some(to.fid);
            }
            Target::Quote(of) => draft.quote = Some(Quoted { fid: of.fid, hash: of.hash, username: of.username }),
            Target::Cast(Some(channel)) => {
                // The channel page's own payload, which the browser holds.
                draft.channel_url = api::peek::<ChannelPayload>(&channel_path(&channel))
                    .and_then(Result::ok)
                    .map(|page| page.data.channel.parent_url.clone())
                    .filter(|url| !url.is_empty())
                    .or_else(|| self.channel_url.get());
                draft.channel = Some(channel);
            }
            Target::Cast(None) => {}
        }
        draft
    }

    /// Post the draft with the reader's key, once they have one, or, with
    /// a time picked, sign it for then and hand it to the Worker: its
    /// mentions are looked up, and a cast that goes out (or is kept) closes
    /// the composer and says so; one that doesn't stays, with why.
    fn post(self, writer: Writer) {
        if self.posting.get_untracked() {
            return;
        }
        // The time is checked before the key is asked for.
        let at = match self.when.get_untracked().map(|value| scheduled_at(&value, now())) {
            None => None,
            Some(Ok(at)) => Some(at),
            Some(Err(why)) => {
                self.error.set(Some(why.into()));
                return;
            }
        };
        let ask = self.hand_off_ask();
        let expect = self.expect();
        writer.when_ready(
            move || {
                self.posting.set(true);
                self.error.set(None);
                let draft = self.draft();
                let saved = self.saved(&draft).to_json();
                let replaces = self.editing.get_untracked();
                // What shows where it will be listed, once it goes out.
                let shown = self.outgoing();
                spawn_local(async move {
                    let fids = fids_of(&mention_names(&draft.text)).await;
                    let cast = draft.new_cast(|name| fids.get(name).copied());
                    let outcome = match (cast.check(), at) {
                        (Err(why), _) => Err(why.to_owned()),
                        (Ok(()), Some(at)) => writer.schedule(cast, at, saved, replaces).await.map(|_| None),
                        (Ok(()), None) => writer.cast(cast).await.map(Some),
                    };
                    self.posting.set(false);
                    match outcome {
                        Ok(hash) => {
                            self.clear();
                            self.close();
                            if let Some(hash) = hash {
                                writer.cast_posted(shown, hash);
                            }
                        }
                        Err(why) => self.error.set(Some(why)),
                    }
                });
            },
            ask,
            Some(expect),
        );
    }

    /// The dialog's title.
    fn title(self) -> &'static str {
        if self.editing.get_untracked().is_some() {
            return "Scheduled cast";
        }
        match self.target.get_untracked() {
            Target::Reply(_) => "Reply",
            Target::Quote(_) => "Quote",
            Target::Cast(_) => "New cast",
        }
    }

    /// What the draft is, in a word: "cast", "reply" or "quote".
    fn noun(self) -> &'static str {
        match self.target.get_untracked() {
            Target::Reply(_) => "reply",
            Target::Quote(_) => "quote",
            Target::Cast(_) => "cast",
        }
    }

    /// The draft as it goes to the Farcaster app: what finds it there once
    /// it's posted, and what shows here meanwhile.
    fn outgoing(self) -> Outgoing {
        let draft = self.draft();
        let text = draft.text.trim().to_owned();
        let (sent, quoting, replying_to) = match self.target.get_untracked() {
            Target::Reply(to) => {
                (Sent::Reply { parent: to.hash.to_ascii_lowercase(), text: text.clone() }, None, Some(to.username))
            }
            Target::Quote(of) => (Sent::Quote { quoted: of.hash.to_ascii_lowercase() }, Some(of.username), None),
            Target::Cast(_) => (Sent::Cast { text: text.clone() }, None, None),
        };
        Outgoing {
            id: 0,
            sent,
            text,
            image: self
                .files
                .with_untracked(|files| files.first().and_then(|f| f.url().map(str::to_owned)))
                .or_else(|| self.gif.with_untracked(|gif| gif.as_ref().map(|g| g.tile.url.clone()))),
            quoting,
            replying_to,
            channel: draft.channel,
            replies_before: self.replies.get_untracked(),
            hash: None,
        }
    }

    /// What the composer does once the draft, handed to the Farcaster app,
    /// is posted there (`true`): lets it go, if it still holds it. Not
    /// posted in time, it has it back, closed and holding no other.
    fn settle(self) -> Settle {
        let files: Vec<Attachment> =
            self.files.with_untracked(|files| files.iter().map(Attachment::kept_copy).collect());
        let held = (self.target.get_untracked(), self.text.get_untracked(), self.gif.get_untracked(), files);
        std::sync::Arc::new(move |posted: bool| {
            let (target, text, gif, files) = &held;
            if posted {
                let holds = self.target.with_untracked(|t| t == target)
                    && self.text.with_untracked(|t| t == text)
                    && self.gif.with_untracked(|g| g == gif)
                    && self.files.with_untracked(|f| f.iter().map(|f| f.id).eq(files.iter().map(|f| f.id)));
                if holds {
                    self.clear();
                    self.close();
                }
            } else if !self.open.get_untracked()
                && self.text.with_untracked(|t| t.trim().is_empty())
                && self.gif.with_untracked(Option::is_none)
                && self.files.with_untracked(Vec::is_empty)
            {
                self.target.set(target.clone());
                self.text.set(text.clone());
                self.gif.set(gif.clone());
                self.files.set(files.clone());
            }
        })
    }

    /// The draft as a hand-off that shows here until it's posted.
    fn expect(self) -> Expect {
        Expect::Posted { cast: self.outgoing(), settle: Some(self.settle()) }
    }

    /// The draft, handed to the Farcaster app: once it has gone, it is
    /// cleared and the composer closes.
    fn hand_off_ask(self) -> Ask {
        let done = Callback::new(move |()| {
            self.clear();
            self.close();
        });
        Ask::post(self.draft().compose_url(), self.noun(), done)
    }
}

impl Default for Composer {
    fn default() -> Self {
        Composer::new()
    }
}

pub fn use_composer() -> Composer {
    expect_context::<Composer>()
}

/// A time to schedule for, to begin with: the first quarter hour an hour
/// or more after `now` (Unix seconds).
fn later(now: i64) -> i64 {
    ((now + 3_600) / 900 + 1) * 900
}

/// When a draft scheduled for `value` (the date-and-time field's, in the
/// reader's time zone) goes out, as of `now`: Unix seconds, or why it can't.
fn scheduled_at(value: &str, now: i64) -> Result<i64, &'static str> {
    let at = from_local_input(value).ok_or("Pick a date and a time.")?;
    if at < now + MIN_AHEAD {
        return Err("Pick a time at least a minute from now.");
    }
    if at > now + MAX_AHEAD {
        return Err("Pick a time within 30 days.");
    }
    Ok(at)
}

/// Ask for the GIF key ahead of a click that opens the composer, so its
/// GIF button is there when it opens, and fetch the composer's code
/// (`composer_view`) if it hasn't come yet. Only a reader signed in gets a
/// key (the Worker answers anyone else 401), so only they ask; the cookie
/// says who is, as an event handler has no context to ask the session.
fn warm() {
    if signin::signed_in() {
        api::prefetch(&gif_key_path());
    }
    spawn_local(async {
        let _ = preload_composer().await;
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

/// The channel whose page is open, if any.
fn channel_in_view(path: &str) -> Option<String> {
    let path = path.strip_prefix(BASE).unwrap_or(path);
    let id = path.strip_prefix("/c/")?.trim_end_matches('/').to_lowercase();
    is_channel_id(&id).then_some(id)
}

/// Whether `path` is a feed with a Cast button: the front page's or a
/// channel's.
fn on_feed(path: &str) -> bool {
    let path = path.strip_prefix(BASE).unwrap_or(path);
    path.is_empty() || path == "/" || channel_in_view(path).is_some()
}

/// Whether `path` is `user`'s own profile, the reader's: `/u/<name>`, or
/// `/u` with `name` its `?name=`, by their username in any case, or
/// `fid:N`.
fn own_profile(path: &str, name: Option<&str>, user: &UserSummary) -> bool {
    let path = path.strip_prefix(BASE).unwrap_or(path).trim_end_matches('/');
    let named = match path.strip_prefix("/u") {
        Some("") => name,
        Some(rest) => rest.strip_prefix('/'),
        None => None,
    };
    named
        .and_then(username_from_param)
        .is_some_and(|named| named == user.username.to_lowercase() || fid_username(&named) == Some(user.fid))
}

/// Whether the page has Cast: the front page's feed, a channel's, and the
/// reader's own profile.
fn use_casts_here() -> Memo<bool> {
    let location = use_location();
    let query = use_query_map();
    let session = use_session();
    Memo::new(move |_| {
        let path = location.pathname.get();
        on_feed(&path)
            || session.user.with(|user| {
                user.as_ref().is_some_and(|user| own_profile(&path, query.read().get("name").as_deref(), user))
            })
    })
}

/// Start a cast, in the channel being read on a channel page.
fn start_cast() -> impl Fn() + Copy + 'static {
    let composer = use_composer();
    let pathname = use_location().pathname;
    move || composer.start(Target::Cast(channel_in_view(&pathname.get_untracked())))
}

/// Cast: a pill in Allthing's accent, lettered in its serif.
#[component]
fn CastPill() -> impl IntoView {
    let start = start_cast();
    view! {
        <button
            type="button"
            class="cast-pill"
            aria-label="Write a cast"
            on:pointerenter=move |_| warm()
            on:focus=move |_| warm()
            on:click=move |_| start()
        >
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=PEN/></svg>
            "Cast"
        </button>
    }
}

/// Cast on a computer: the pill in the site header, before the account,
/// always in view as the page scrolls, on the pages that have Cast (the
/// front page's feed, a channel's, the reader's own profile). On the others
/// its place is kept, unseen and out of reach, so the header doesn't move
/// from page to page. On a phone, `CastFab` instead.
#[component]
pub fn HeaderCast() -> impl IntoView {
    let here = use_casts_here();
    view! {
        <span class="header-cast" class:here=move || here.get()>
            <CastPill/>
        </span>
    }
}

/// Whether `element` takes typing, which on a phone brings up the
/// keyboard: a text field, a text area or editable text.
fn takes_typing(element: &web_sys::Element) -> bool {
    match element.tag_name().as_str() {
        "TEXTAREA" => true,
        "INPUT" => !matches!(
            element.get_attribute("type").unwrap_or_default().to_ascii_lowercase().as_str(),
            "button" | "checkbox" | "color" | "file" | "hidden" | "image" | "radio" | "range" | "reset" | "submit"
        ),
        _ => element.dyn_ref::<HtmlElement>().is_some_and(HtmlElement::is_content_editable),
    }
}

/// Cast on a phone: a round button floating at the bottom right of the
/// pages that have Cast, within the thumb's reach. The page leaves room
/// after its end for it, and it steps out of the way of a dialog, the
/// lightbox and a popover (`style/allthing.css`), and of the keyboard,
/// while a text box has focus (`away`).
#[component]
pub fn CastFab() -> impl IntoView {
    let start = start_cast();
    let shown = use_casts_here();
    let typing = RwSignal::new(false);
    // Focus moving in, or out (to where it lands, once it has).
    let focused = move || typing.set(document().active_element().is_some_and(|el| takes_typing(&el)));
    let moved_in = window_event_listener(ev::focusin, move |_| focused());
    let moved_out = window_event_listener(ev::focusout, move |_| {
        set_timeout(focused, Duration::ZERO);
    });
    on_cleanup(move || {
        moved_in.remove();
        moved_out.remove();
    });
    move || {
        shown.get().then(|| {
            view! {
                <button
                    type="button"
                    class="cast-fab"
                    class:away=move || typing.get()
                    aria-label="Write a cast"
                    on:pointerdown=move |_| warm()
                    on:focus=move |_| warm()
                    on:click=move |_| start()
                >
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=PEN/></svg>
                </button>
            }
        })
    }
}

/// Reply, under a post or a reply.
#[component]
pub fn ReplyButton(cast: Cast) -> impl IntoView {
    let composer = use_composer();
    let to = Other::of(&cast);
    let replies = cast.replies;
    let label = format!("Reply to @{}", to.username);
    view! {
        <button
            type="button"
            class="reply-button"
            aria-label=label
            on:pointerenter=move |_| warm()
            on:focus=move |_| warm()
            on:click=move |_| {
                composer.replies.set(replies);
                composer.start(Target::Reply(to.clone()));
            }
        >
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=REPLY/></svg>
            <span class="button-label">"Reply"</span>
        </button>
    }
}

/// Quote, under a post or a reply: a cast that embeds this one.
#[component]
pub fn QuoteButton(cast: Cast) -> impl IntoView {
    let composer = use_composer();
    let of = Other::of(&cast);
    let label = format!("Quote @{}'s cast", of.username);
    view! {
        <button
            type="button"
            class="reply-button quote-button"
            aria-label=label
            on:pointerenter=move |_| warm()
            on:focus=move |_| warm()
            on:click=move |_| composer.start(Target::Quote(of.clone()))
        >
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=QUOTE/></svg>
            <span class="button-label">"Quote"</span>
        </button>
    }
}

/// The composer's dialog: the draft's text, its GIF, the GIF picker, and
/// the button that posts it (or, where Allthing can't, the link that opens
/// it in the Farcaster app).
#[component]
pub fn ComposerDialog() -> impl IntoView {
    let composer = use_composer();
    let session = use_session();
    let location = use_location();
    let dialog = NodeRef::<leptos::html::Div>::new();
    // A history navigation, or signing out, closes it. Signed out, it
    // takes no files.
    Effect::new(move |_| {
        let _ = (location.pathname.get(), location.search.get());
        if session.user.with(Option::is_none) && composer.limits.with_untracked(Option::is_some) {
            composer.limits.set(None);
        }
        if composer.open.get_untracked() {
            composer.close();
        }
    });
    Effect::new(move |_| {
        let open = composer.open.get();
        if let Some(root) = document().document_element() {
            let _ = root.class_list().toggle_with_force("compose-open", open);
        }
        // Whether files can go with the draft, asked while the composer's
        // code comes the first time (`composer_bar` reads it).
        if open && session.user.with_untracked(Option::is_some) {
            api::prefetch(MEDIA_PATH);
        }
    });
    let keys = window_event_listener(ev::keydown, move |ev| {
        // The key dialog, over the composer for a first post, or the
        // hand-off to the Farcaster app, has the keys.
        let over = document()
            .document_element()
            .is_some_and(|root| root.class_list().contains("key-open") || root.class_list().contains("handoff-open"));
        if !composer.open.get_untracked() || over {
            return;
        }
        match ev.key().as_str() {
            // The picker first, then the dialog.
            "Escape" => {
                ev.prevent_default();
                match composer.picker.get_untracked().button(composer.tab.get_untracked()) {
                    Some(button) => {
                        composer.picker.set(Picker::Closed);
                        focus_soon(button);
                    }
                    None => composer.close(),
                }
            }
            // Focus stays in the dialog.
            "Tab" => {
                if let Some(root) = dialog.get_untracked() {
                    trap_tab(&root, &ev, "button:not([disabled]), a[href], [contenteditable], input:not([type=file])");
                }
            }
            _ => {}
        }
    });
    on_cleanup(move || keys.remove());
    move || {
        composer.open.get().then(|| {
            // KLIPY's key, once the bar has asked for it (`composer_bar`).
            let gif_key = RwSignal::new(None::<String>);
            // Only the field's showing, not its value, redraws the row.
            let scheduling = Memo::new(move |_| composer.when.with(Option::is_some));
            let text = NodeRef::<leptos::html::Div>::new();
            // The draft it holds already, with the caret at its end.
            Effect::new(move |_| {
                if let Some(root) = text.get() {
                    let draft = composer.text.get_untracked();
                    if editor::text_of(&root) != draft {
                        editor::show(&root, &draft);
                    }
                    let _ = root.focus();
                    let end = draft.encode_utf16().count();
                    editor::place(&root, end, end);
                }
            });
            // Where the caret was when the text lost focus, or after the last
            // emoji put in: a tap elsewhere can move an unfocused text box's
            // own selection to its start.
            let caret = StoredValue::new(None::<(usize, usize)>);
            let (placeholder, context) = match composer.target.get_untracked() {
                Target::Reply(to) => (
                    "Write your reply",
                    Some(view! {
                        <p class="compose-context">
                            "Replying to "<strong>"@"{to.username}</strong>
                            {(!to.excerpt.is_empty()).then(|| view! { <span class="compose-excerpt"><ExcerptText text=to.excerpt/></span> })}
                        </p>
                    }.into_any()),
                ),
                Target::Quote(of) => (
                    "Add your thoughts",
                    Some(view! {
                        <p class="compose-context">
                            "Quoting "<strong>"@"{of.username}</strong>
                            {(!of.excerpt.is_empty()).then(|| view! { <span class="compose-excerpt"><ExcerptText text=of.excerpt/></span> })}
                        </p>
                    }.into_any()),
                ),
                Target::Cast(channel) => (
                    "What's happening?",
                    channel.map(|id| view! { <p class="compose-context">"In "<strong>"/"{id}</strong></p> }.into_any()),
                ),
            };
            // The text is here at once, to type in, the first time too; what
            // goes around it comes with the composer's code (`composer_extras`,
            // `composer_bar`). The extras say nothing if it can't come: the
            // bar does.
            let extras = async move {
                Ok(composer_extras(composer, text, caret, gif_key, scheduling).await.unwrap_or_else(|_| ().into_any()))
            };
            // Files dragged over the composer, which takes them dropped
            // anywhere on it, where this server takes files; nested elements
            // each say they're entered and left, so it counts them.
            let entered = StoredValue::new(0i32);
            let files_in = move |ev: &ev::DragEvent| {
                composer.takes_files()
                    && ev
                        .data_transfer()
                        .is_some_and(|data| data.types().includes(&wasm_bindgen::JsValue::from_str("Files"), 0))
            };
            // A paste of files attaches them, where they can go.
            let pasted = Callback::new(move |data: web_sys::DataTransfer| composer.take_files(&data, false));
            view! {
                <div
                    class="modal-backdrop"
                    on:click=move |ev: ev::MouseEvent| {
                        if ev.target() == ev.current_target() {
                            composer.close();
                        }
                    }
                    on:dragenter=move |ev: ev::DragEvent| {
                        if files_in(&ev) {
                            ev.prevent_default();
                            entered.update_value(|n| *n += 1);
                            composer.dragging.set(true);
                        }
                    }
                    on:dragover=move |ev: ev::DragEvent| {
                        if files_in(&ev) {
                            ev.prevent_default();
                            if let Some(data) = ev.data_transfer() {
                                data.set_drop_effect("copy");
                            }
                            if !composer.dragging.get_untracked() {
                                composer.dragging.set(true);
                            }
                        }
                    }
                    on:dragleave=move |ev: ev::DragEvent| {
                        if files_in(&ev) {
                            entered.update_value(|n| *n -= 1);
                            if entered.get_value() <= 0 {
                                entered.set_value(0);
                                composer.dragging.set(false);
                            }
                        }
                    }
                    // Never opened in the tab in the composer's place.
                    on:drop=move |ev: ev::DragEvent| {
                        if let Some(data) = ev.data_transfer().filter(|_| files_in(&ev)) {
                            ev.prevent_default();
                            entered.set_value(0);
                            composer.dragging.set(false);
                            composer.take_files(&data, true);
                        }
                    }
                >
                    <div class="modal compose-modal" role="dialog" aria-modal="true" aria-labelledby="compose-title" node_ref=dialog>
                        {move || composer.dragging.get().then(|| view! {
                            <div class="compose-drop" aria-hidden="true"><p>"Drop images or videos to attach them"</p></div>
                        })}
                        <header class="modal-head">
                            <h2 id="compose-title">{composer.title()}</h2>
                            <button type="button" class="modal-close" aria-label="Close" on:click=move |_| composer.close()>"×"</button>
                        </header>
                        // A picker open makes the text smaller, to fit both.
                        <div class="modal-body compose-body" class:picking=move || composer.picker.get() != Picker::Closed>
                            {context}
                            <ComposeText node_ref=text text=composer.text caret label=placeholder files=pasted/>
                            {crate::split::lazy_view(|| (), extras)}
                        </div>
                        // Below what scrolls, so the picker never hides it.
                        {crate::split::lazy_view(pending_bar, composer_bar(composer, gif_key, scheduling))}
                    </div>
                </div>
            }
        })
    }
}

/// The composer's bar while its code comes, the first time it opens.
fn pending_bar() -> impl IntoView {
    view! {
        <footer class="compose-foot" aria-busy="true">
            <div class="compose-bar"><span class="button compose-post compose-loading">"Loading…"</span></div>
        </footer>
    }
}

/// What the composer shows under the text: its GIF or sticker, the GIF,
/// emoji and sticker pickers, when it goes out, and why it didn't. With
/// `composer_bar`, the composer's part of the app, fetched the first time
/// the composer opens, or as a Cast, Reply or Quote button is pointed at or
/// focused (`warm`; `crate::split`).
#[wasm_split(composer, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_composer))]
fn composer_extras(
    composer: Composer,
    text: NodeRef<leptos::html::Div>,
    caret: StoredValue<Option<(usize, usize)>>,
    gif_key: RwSignal<Option<String>>,
    scheduling: Memo<bool>,
) -> Result<AnyView, SplitLoaderError> {
    attach::take_incoming(composer);
    // With two embeds already (files, and the cast it quotes), a GIF or a
    // sticker can't go too.
    let full = move || {
        let full = !composer.room_for_gif();
        if full {
            composer.note.set(Some(EMBEDS_FULL.to_owned()));
        }
        full
    };
    let picked = Callback::new(move |gif: Gif| {
        if !full() {
            composer.gif.set(Some(gif));
        }
        composer.picker.set(Picker::Closed);
        focus_soon(".compose-text");
    });
    // An emoji's shortcode goes in at the caret, or in place of what
    // is selected, and the caret after it.
    let insert = Callback::new(move |(emoji, again): Pick| {
        let Some(root) = text.get_untracked() else { return };
        let value = composer.text.get_untracked();
        let focused = document().active_element().is_some_and(|el| el.is_same_node(Some(&root)));
        let end = value.encode_utf16().count();
        let (from, to) = if focused { editor::caret_in(&root) } else { caret.get_value() }.unwrap_or((end, end));
        let (value, at) = emoji::insert(&value, from, to, emoji);
        editor::show(&root, &value);
        composer.text.set(value);
        caret.set_value(Some((at, at)));
        emoji_picker::used(emoji);
        // Picked to stay in the picker, the text box isn't focused, as a
        // caret placed in it would be, so a phone's keyboard stays down.
        if !again {
            composer.picker.set(Picker::Closed);
            let _ = root.focus();
            editor::place(&root, at, at);
        } else if focused {
            editor::place(&root, at, at);
        }
    });
    // A sticker goes with the cast as its GIF does, in the GIF's place.
    let stick = Callback::new(move |sticker: &'static Sticker| {
        if full() {
            composer.picker.set(Picker::Closed);
            focus_soon(".compose-text");
            return;
        }
        emoji_picker::used_sticker(sticker);
        let (width, height) = (u32::from(sticker.width), u32::from(STICKER_HEIGHT));
        composer.gif.set(Some(Gif {
            slug: sticker.code.to_owned(),
            title: format!("Sticker: {}", sticker.label()),
            tile: Rendition { url: href(&sticker.file()), width, height },
            embed: Rendition { url: sticker.embed(), width, height },
        }));
        composer.picker.set(Picker::Closed);
        focus_soon(".compose-text");
    });
    Ok(view! {
        <Attached composer/>
        {move || composer.gif.get().map(|gif| view! {
            <figure class="compose-gif" class:compose-sticker=emoji::sticker_of(&gif.embed.url).is_some()>
                <img src=gif.tile.url.clone() width=gif.tile.width height=gif.tile.height alt=gif.title.clone()/>
                <button type="button" class="compose-gif-remove" aria-label=if emoji::sticker_of(&gif.embed.url).is_some() { "Remove the sticker" } else { "Remove the GIF" } on:click=move |_| {
                    composer.gif.set(None);
                    focus_soon(".compose-text");
                }>"×"</button>
            </figure>
        })}
        {move || (composer.picker.get() == Picker::Gifs).then(|| gif_key.get()).flatten().map(|key| view! { <GifPicker key picked/> })}
        {move || (composer.picker.get() == Picker::Emoji).then(|| view! { <EmojiPicker tab=composer.tab picked=insert stuck=stick/> })}
        {move || scheduling.get().then(|| {
            let zone = time_zone().map(|zone| format!("Your time, {zone}")).unwrap_or_else(|| "Your time".into());
            view! {
                <div class="compose-when">
                    <label for="compose-at">"Goes out"</label>
                    <input
                        id="compose-at"
                        class="compose-at"
                        type="datetime-local"
                        min=local_input(now() + MIN_AHEAD + 59)
                        max=local_input(now() + MAX_AHEAD)
                        prop:value=move || composer.when.get().unwrap_or_default()
                        on:input=move |ev| composer.when.set(Some(event_target_value(&ev)))
                    />
                    <span class="compose-zone muted small">{zone}</span>
                    <a class="compose-scheduled small" href=scheduled_href()>"Scheduled casts"</a>
                </div>
            }
        })}
        // There before anything is said in it, for it to be heard.
        <p class="attach-note" role="status">{move || composer.note.get()}</p>
        {move || composer.error.get().map(|why| view! { <p class="compose-error" role="alert">{why}</p> })}
    }
    .into_any())
}

/// The composer's bar: the emoji, sticker, GIF and schedule buttons, the
/// count, and Cast (or Post in Farcaster). In the composer's part, with
/// `composer_extras`.
#[wasm_split(composer, wasm_split_path = ::leptos::wasm_split, fallible)]
fn composer_bar(
    composer: Composer,
    gif_key: RwSignal<Option<String>>,
    scheduling: Memo<bool>,
) -> Result<AnyView, SplitLoaderError> {
    // The GIF key, for a reader signed in, the only one the Worker gives
    // one to: kept in the client cache for 5 minutes, and by the browser for
    // an hour.
    let session = use_session();
    let key = LocalResource::new(move || {
        let signed_in = session.user.with(Option::is_some);
        async move { if signed_in { api::get::<GifKey>(&gif_key_path()).await.ok() } else { None } }
    });
    Effect::new(move |_| {
        gif_key.set(key.get().flatten().map(|k| k.data.key.clone()).filter(|k| !k.is_empty()));
    });
    // Whether files can go with a cast: asked as the composer opens, for a
    // reader signed in (kept 5 minutes).
    let media = LocalResource::new(move || {
        let signed_in = session.user.with(Option::is_some);
        async move { if signed_in { Some(api::get::<MediaLimits>(MEDIA_PATH).await) } else { None } }
    });
    // Only a 200 shows the attach button: not set up (404), signed out or
    // not answered, the composer is as it was without files.
    Effect::new(move |_| {
        if let Some(answer) = media.get() {
            composer.limits.set(answer.and_then(Result::ok).map(|fetched| *fetched.data));
        }
    });
    let room = Memo::new(move |_| composer.room_for_gif());
    let writer = use_writer();
    // Posted from here wherever the Worker endorses keys, with no
    // way round it; the Farcaster app only where it doesn't.
    let posts_here = Memo::new(move |_| writer.map_or(Some(false), |w| w.posts_here()));
    // The text's length, in bytes, as a cast counts it.
    let used = Memo::new(move |_| composer.text.with(|t| t.trim().len()));
    Ok(view! {
        <footer class="compose-foot">
            <div class="compose-bar">
                <AttachButton composer/>
                <button
                    type="button"
                    class="emoji-button"
                    aria-label="Add an emoji"
                    aria-expanded=move || composer.showing(Tab::Emoji).to_string()
                    aria-controls="emoji-picker"
                    on:click=move |_| composer.toggle_tab(Tab::Emoji)
                >
                    <img src=href(&emoji::find("pepe_smirk").unwrap_or(&emoji::PACK[0]).still()) alt="" width="22" height="22"/>
                </button>
                <button
                    type="button"
                    class="sticker-button"
                    aria-label="Add a sticker"
                    aria-expanded=move || composer.showing(Tab::Stickers).to_string()
                    aria-controls="emoji-picker"
                    aria-disabled=move || (!room.get()).then_some("true")
                    aria-description=move || (!room.get()).then_some(EMBEDS_FULL)
                    on:click=move |_| {
                        if room.get_untracked() {
                            composer.toggle_tab(Tab::Stickers);
                        } else {
                            composer.note.set(Some(EMBEDS_FULL.to_owned()));
                        }
                    }
                >
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=STICKER/></svg>
                </button>
                {move || gif_key.get().map(|_| view! {
                    <button
                        type="button"
                        class="gif-button"
                        aria-label="Add a GIF"
                        aria-expanded=move || (composer.picker.get() == Picker::Gifs).to_string()
                        aria-controls="gif-picker"
                        aria-disabled=move || (!room.get()).then_some("true")
                        aria-description=move || (!room.get()).then_some(EMBEDS_FULL)
                        on:click=move |_| {
                            if room.get_untracked() {
                                composer.toggle(Picker::Gifs);
                            } else {
                                composer.note.set(Some(EMBEDS_FULL.to_owned()));
                            }
                        }
                    >"GIF"</button>
                })}
                {move || (posts_here.get() == Some(true)).then(|| view! {
                    <button
                        type="button"
                        class="schedule-button"
                        aria-label="Schedule for later"
                        aria-pressed=move || scheduling.get().to_string()
                        disabled=move || composer.editing.with(Option::is_some)
                        on:click=move |_| composer.toggle_when()
                    >
                        <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.2"/><path d=CLOCK/></svg>
                    </button>
                })}
                {move || (used.get() > COUNT_FROM).then(|| view! {
                    <span class="compose-count" class:over=move || { used.get() > MAX_CAST_BYTES }>
                        {move || format!("{} / {}", used.get(), MAX_CAST_BYTES)}
                    </span>
                })}
                {move || match (posts_here.get(), writer) {
                    (Some(false), _) | (_, None) => view! { <PostLink composer/> }.into_any(),
                    (_, Some(writer)) => view! { <PostButton composer writer/> }.into_any(),
                }}
            </div>
        </footer>
    }
    .into_any())
}

/// Cast, Reply or Quote, with the reader's key: their first post opens the
/// dialog that approves one. It waits while the Worker is asked whether it
/// endorses keys.
#[component]
fn PostButton(composer: Composer, writer: Writer) -> impl IntoView {
    let blocked = Memo::new(move |_| {
        let empty = composer.text.with(|t| t.trim().is_empty())
            && composer.gif.with(Option::is_none)
            && composer.files.with(Vec::is_empty)
            && !matches!(composer.target.get(), Target::Quote(_));
        empty
            || composer.text.with(|t| t.trim().len() > MAX_CAST_BYTES)
            || writer.posts_here().is_none()
            || composer.uploading()
            || composer.upload_failed()
    });
    let label = move || {
        let later = composer.when.with(Option::is_some);
        if composer.uploading() {
            return "Uploading…";
        }
        match (composer.posting.get(), later, composer.target.get()) {
            (true, true, _) => "Scheduling…",
            (true, false, _) => "Posting…",
            (false, true, _) if composer.editing.with(Option::is_some) => "Save",
            (false, true, _) => "Schedule",
            (false, false, Target::Reply(_)) => "Reply",
            (false, false, Target::Quote(_)) => "Quote",
            (false, false, Target::Cast(_)) => "Cast",
        }
    };
    view! {
        <button
            type="button"
            class="button compose-post"
            disabled=move || blocked.get() || composer.posting.get()
            aria-busy=move || composer.posting.get().then_some("true")
            on:click=move |_| composer.post(writer)
        >
            {label}
        </button>
    }
}

/// "Post in Farcaster": on a phone, the draft's compose link, which opens
/// the Farcaster app; on a computer, the hand-off, with the link as a QR
/// code for the reader's phone and on farcaster.xyz. The draft is the app's
/// from there on.
#[component]
fn PostLink(composer: Composer) -> impl IntoView {
    let empty = Memo::new(move |_| composer.draft().is_empty());
    // Files on their way, or one that didn't go: not yet.
    let waiting = Memo::new(move |_| match (composer.uploading(), composer.upload_failed()) {
        (true, _) => Some("Uploading…"),
        (false, true) => Some("Post in Farcaster"),
        _ => None,
    });
    let hand_off = use_hand_off();
    let writer = use_writer();
    move || {
        if let Some(label) = waiting.get() {
            view! { <button type="button" class="button compose-post" disabled=true>{label}</button> }.into_any()
        } else if empty.get() {
            view! { <button type="button" class="button compose-post" disabled=true>"Post in Farcaster"</button> }
                .into_any()
        } else if can_hover() {
            view! {
                <button
                    type="button"
                    class="button compose-post"
                    aria-haspopup="dialog"
                    on:click=move |_| match writer {
                        Some(writer) => writer.hand_off(composer.hand_off_ask(), Some(composer.expect())),
                        None => hand_off.open(composer.hand_off_ask()),
                    }
                >
                    "Post in Farcaster"
                </button>
            }
            .into_any()
        } else {
            view! {
                <a
                    class="button compose-post"
                    href=move || composer.draft().compose_url()
                    target="_blank"
                    rel="noopener noreferrer"
                    on:click=move |_| {
                        if let Some(writer) = writer {
                            writer.went_to_app(composer.expect(), composer.draft().compose_url());
                        }
                        // After the link has opened.
                        set_timeout(
                            move || {
                                composer.clear();
                                composer.close();
                            },
                            Duration::ZERO,
                        );
                    }
                >
                    "Post in Farcaster"
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d=OUT/></svg>
                </a>
            }
            .into_any()
        }
    }
}

/// What the picker shows.
#[derive(Clone, PartialEq)]
enum Shown {
    Loading,
    /// Its GIFs are in `gifs`; whether KLIPY has more.
    Ready {
        more: bool,
    },
    Failed(Unread),
}

/// Why KLIPY's answer didn't come.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Unread {
    /// The key's requests for the hour are spent (KLIPY limits a key in
    /// testing to 100 an hour, for every reader together).
    Busy,
    Failed,
}

/// The GIF picker: what's trending on KLIPY, or its results for what is
/// typed, in KLIPY's order. Picking one attaches it to the draft.
#[component]
fn GifPicker(key: String, picked: Callback<Gif>) -> impl IntoView {
    let gifs = RwSignal::new(Vec::<Gif>::new());
    let shown = RwSignal::new(Shown::Loading);
    // The query `gifs` answers, and the page they reach.
    let query = StoredValue::new(String::new());
    let page = StoredValue::new(1u32);
    // A new query makes answers still on their way for an older one moot,
    // and gives its tiles new keys.
    let revision = RwSignal::new(0u64);
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let input = NodeRef::<leptos::html::Input>::new();
    // Into the search box where there's a keyboard to type with; on a
    // phone, the keyboard would cover the GIFs.
    Effect::new(move |_| {
        if let Some(input) = input.get().filter(|_| can_hover()) {
            let _ = input.focus();
        }
    });
    let load = Callback::new(move |next: u32| {
        let requested = revision.get_untracked();
        let url = gifs_url(&key, &query.get_value(), next);
        spawn_local(async move {
            let found = klipy(&url).await;
            if revision.try_get_untracked() != Some(requested) {
                return;
            }
            match found {
                Ok(found) => {
                    let (more, more_after) = (found.0.clone(), found.1);
                    gifs.update(|g| {
                        if next == 1 {
                            *g = more;
                        } else {
                            g.extend(more);
                        }
                    });
                    page.set_value(next);
                    shown.set(Shown::Ready { more: more_after });
                }
                // A later page that fails leaves what is shown, and its button.
                Err(_) if next > 1 => shown.set(Shown::Ready { more: true }),
                Err(why) => shown.set(Shown::Failed(why)),
            }
        });
    });
    let search = move |q: String| {
        query.set_value(q);
        revision.update(|r| *r = r.wrapping_add(1));
        shown.set(Shown::Loading);
        load.run(1);
    };
    // What's trending, at once.
    load.run(1);
    let on_input = move |ev: ev::Event| {
        if let Some(handle) = timer.try_update_value(Option::take).flatten() {
            handle.clear();
        }
        let q = event_target_value(&ev).trim().to_owned();
        let n = q.chars().count();
        if q == query.get_value() || (n > 0 && n < MIN_QUERY) {
            return;
        }
        timer.set_value(set_timeout_with_handle(move || search(q), DEBOUNCE).ok());
    };
    // Enter searches at once, without waiting for typing to pause, and on
    // a phone puts the keyboard away so the GIFs show.
    let on_keydown = move |ev: ev::KeyboardEvent| {
        if ev.key() != "Enter" {
            return;
        }
        ev.prevent_default();
        if let Some(handle) = timer.try_update_value(Option::take).flatten() {
            handle.clear();
        }
        let q = event_target_value(&ev).trim().to_owned();
        let n = q.chars().count();
        if q != query.get_value() && !(n > 0 && n < MIN_QUERY) {
            search(q);
        }
        if let Some(input) = input.get_untracked().filter(|_| !can_hover()) {
            let _ = input.blur();
        }
    };
    on_cleanup(move || {
        if let Some(Some(handle)) = timer.try_get_value() {
            handle.clear();
        }
    });
    let heading = move || {
        let q = query.get_value();
        let _ = revision.get();
        if q.is_empty() { "Trending".to_owned() } else { format!("GIFs for “{q}”") }
    };
    view! {
        <section class="gif-picker" id="gif-picker" aria-label="GIFs from KLIPY">
            <input
                class="gif-search"
                type="search"
                node_ref=input
                placeholder=SEARCH_PLACEHOLDER
                aria-label="Search GIFs"
                autocomplete="off"
                enterkeyhint="search"
                on:input=on_input
                on:keydown=on_keydown
            />
            <p class="gif-heading muted small" aria-live="polite">{heading}</p>
            <div class="gif-results">
                {move || match shown.get() {
                    Shown::Loading => view! {
                        <ul class="gif-grid" aria-busy="true">
                            {(0..9).map(|_| view! { <li class="gif-tile placeholder"></li> }).collect_view()}
                        </ul>
                    }
                    .into_any(),
                    Shown::Failed(why) => view! {
                        <p class="gif-note">
                            {match why {
                                Unread::Busy => "Too many GIF searches on Allthing just now. ",
                                Unread::Failed => "GIFs couldn't load just now. ",
                            }}
                            <button type="button" class="link-button" on:click=move |_| search(query.get_value())>"Try again"</button>
                        </p>
                    }
                    .into_any(),
                    Shown::Ready { .. } if gifs.with(Vec::is_empty) => view! { <p class="gif-note">"No GIFs found."</p> }.into_any(),
                    Shown::Ready { more } => view! {
                        <ul class="gif-grid">
                            <For
                                each=move || {
                                    let r = revision.get();
                                    gifs.get().into_iter().enumerate().map(move |(i, gif)| (r, i, gif)).collect::<Vec<_>>()
                                }
                                key=|(r, i, _)| (*r, *i)
                                children=move |(_, _, gif)| view! { <GifTile gif picked/> }
                            />
                        </ul>
                        {more.then(|| view! {
                            <button type="button" class="button gif-more" on:click=move |_| load.run(page.get_value() + 1)>"More GIFs"</button>
                        })}
                    }
                    .into_any(),
                }}
            </div>
            <p class="gif-credit">"Powered by KLIPY"</p>
        </section>
    }
}

/// A GIF in the picker, whole: its tile grows from a basis of its shape
/// (`--shape`, width over height) at the row's height, so a row of them
/// shares one height and fills its width.
#[component]
fn GifTile(gif: Gif, picked: Callback<Gif>) -> impl IntoView {
    let (src, width, height, title) = (gif.tile.url.clone(), gif.tile.width, gif.tile.height, gif.title.clone());
    let shape = if width > 0 && height > 0 { f64::from(width) / f64::from(height) } else { 1.0 };
    view! {
        <li style=format!("--shape: {shape:.3}")>
            <button type="button" class="gif-tile" aria-label=title on:click=move |_| picked.run(gif.clone())>
                // In view as the picker opens: not left to lazy loading.
                <img src=src width=width height=height alt="" decoding="async"/>
            </button>
        </li>
    }
}

/// A page of KLIPY's results, asked for from the browser, as KLIPY
/// requires, and kept for this visit.
async fn klipy(url: &str) -> Result<Found, Unread> {
    let now = js_sys::Date::now();
    let kept = PAGES.with(|p| p.borrow().get(url).filter(|(at, _)| now - at < KEEP_MS).map(|(_, page)| page.clone()));
    if let Some(page) = kept {
        return Ok(page);
    }
    let failed = |_| Unread::Failed;
    let window = web_sys::window().ok_or(Unread::Failed)?;
    let response: web_sys::Response =
        JsFuture::from(window.fetch_with_str(url)).await.map_err(failed)?.dyn_into().map_err(failed)?;
    match response.status() {
        200 => {}
        429 => return Err(Unread::Busy),
        _ => return Err(Unread::Failed),
    }
    let text = JsFuture::from(response.text().map_err(failed)?).await.map_err(failed)?.as_string();
    let page: GifPage = text.and_then(|text| serde_json::from_str(&text).ok()).ok_or(Unread::Failed)?;
    if !page.result {
        return Err(Unread::Failed);
    }
    let page = Rc::new(page.gifs());
    PAGES.with(|p| {
        let mut p = p.borrow_mut();
        p.retain(|_, (at, _)| now - *at < KEEP_MS);
        p.insert(url.to_owned(), (now, page.clone()));
    });
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_is_on_the_feeds_and_the_readers_own_profile() {
        let reader = UserSummary { fid: 3, username: "reader.eth".into(), display_name: String::new(), pfp_url: None };
        for path in ["/", "", "/c/dev", "/c/dev/"] {
            assert!(on_feed(path), "{path}");
        }
        for path in ["/p/0x01", "/u/reader.eth", "/search", "/channels", "/scheduled", "/dau"] {
            assert!(!on_feed(path), "{path}");
        }
        for (path, name) in [
            ("/u/reader.eth", None),
            ("/u/Reader.ETH/", None),
            ("/u/%40reader.eth", None),
            ("/u/fid:3", None),
            ("/u/fid%3A3", None),
            ("/u", Some("reader.eth")),
            ("/u/", Some("fid:3")),
        ] {
            assert!(own_profile(path, name, &reader), "{path} {name:?}");
        }
        for (path, name) in [
            ("/u/bob", None),
            ("/u/fid:4", None),
            ("/u", Some("bob")),
            ("/u", None),
            ("/users/reader.eth", None),
            ("/p/reader.eth", None),
            ("/", Some("reader.eth")),
        ] {
            assert!(!own_profile(path, name, &reader), "{path} {name:?}");
        }
    }

    #[test]
    fn a_cast_from_a_channel_page_goes_to_that_channel() {
        assert_eq!(channel_in_view("/c/dev"), Some("dev".to_owned()));
        assert_eq!(channel_in_view("/c/Dev/"), Some("dev".to_owned()));
        assert_eq!(channel_in_view("/"), None);
        assert_eq!(channel_in_view("/channels"), None);
        assert_eq!(channel_in_view("/c/"), None);
    }
}

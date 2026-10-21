// SPDX-License-Identifier: AGPL-3.0-or-later
//! Composing a cast. A draft (its text, a GIF, the channel it goes to, the
//! cast it answers or quotes) becomes the cast the reader's key signs
//! ([`Draft::new_cast`]); where Allthing endorses no keys, it opens in the
//! Farcaster app instead, ready to post, through farcaster.xyz's compose
//! link ([`Draft::compose_url`]). Likes, recasts, follows and deletions go
//! there the same way, to the cast or the profile ([`cast_url`],
//! [`profile_url`]): the links Allthing hands a write to ([`is_handoff_link`]).
//!
//! GIFs come from KLIPY (<https://docs.klipy.com>). KLIPY asks that its API
//! and its media be reached from the reader's browser, never through a
//! partner's server, and that results keep the order and composition it
//! returns them in. The browser therefore asks KLIPY itself, with the app
//! key the Worker hands a signed-in session ([`gif_key_path`]), and shows
//! every result in order.

use serde::{Deserialize, Serialize};

use crate::api::API_PREFIX;
use crate::embeds::{ImageSize, resized_image_url};
use crate::message::{CastId, Embed, MAX_CAST_BYTES, NewCast, Parent, SHORT_CAST_BYTES};
use crate::model::{Cast, Embed as CastEmbed};
use crate::params::percent_encode;
use crate::text::{split_mentions, text_start, truncate_title};

/// Where the Farcaster app opens a draft, ready to post.
pub const COMPOSE_URL: &str = "https://farcaster.xyz/~/compose";
/// KLIPY's API.
pub const KLIPY_API: &str = "https://api.klipy.com/api/v1";
/// GIFs asked for at a time (KLIPY allows 8 to 50).
pub const GIFS_PER_PAGE: u32 = 24;
/// The longest embed a cast takes, in bytes.
pub const MAX_EMBED_BYTES: usize = 256;
/// The search box's placeholder, which KLIPY requires.
pub const SEARCH_PLACEHOLDER: &str = "Search KLIPY";

/// The Worker's answer to [`gif_key_path`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GifKey {
    pub key: String,
}

/// KLIPY's app key, for a signed-in browser; 404 when GIF search isn't set
/// up (no `KLIPY_API_KEY` secret).
pub fn gif_key_path() -> String {
    format!("{API_PREFIX}/gifs/key")
}

/// A cast to post: signed with the reader's key ([`Draft::new_cast`]), or
/// opened in the Farcaster app ([`Draft::compose_url`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    /// Media and links to embed: a cast takes two, a quoted cast included.
    pub embeds: Vec<String>,
    /// The cast it quotes.
    pub quote: Option<Quoted>,
    /// The cast it replies to, and that cast's author.
    pub parent_hash: Option<String>,
    pub parent_fid: Option<u64>,
    /// The channel it goes to (an id, such as "farcaster"), and the parent
    /// URL that puts a cast in it. A reply goes where its parent is.
    pub channel: Option<String>,
    pub channel_url: Option<String>,
}

/// A cast a draft quotes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quoted {
    pub fid: u64,
    pub hash: String,
    pub username: String,
}

impl Quoted {
    /// Its link in the Farcaster app, which quotes it when a draft opens
    /// there.
    pub fn url(&self) -> String {
        cast_url(&self.username, &self.hash)
    }
}

/// How long a draft is, as the composer's ring shows it, as the Farcaster
/// app's does ([`length`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    /// A ring this full, from 0 to 1: toward a cast's 320 bytes, then
    /// toward a long cast's 1,024. Amber (`alert`) under [`ALERT_LEFT`]
    /// bytes left.
    Ring { full: f64, alert: bool },
    /// The bytes left, said as a number under [`NUMBER_LEFT`]: below zero
    /// past the limit.
    Left(i64),
}

/// Bytes left under which the ring turns amber.
pub const ALERT_LEFT: i64 = 75;
/// Bytes left under which the ring gives way to their number.
pub const NUMBER_LEFT: i64 = 25;

/// How long a draft of `bytes` is ([`Length`]).
pub fn length(bytes: usize) -> Length {
    let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
    let (short, long) = (SHORT_CAST_BYTES as i64, MAX_CAST_BYTES as i64);
    if bytes < short {
        Length::Ring { full: bytes as f64 / short as f64, alert: false }
    } else if long - bytes >= NUMBER_LEFT {
        Length::Ring { full: bytes as f64 / long as f64, alert: long - bytes < ALERT_LEFT }
    } else {
        Length::Left(long - bytes)
    }
}

/// What a screen reader hears of a draft's length: something only where it
/// crosses a line (a long cast, then the last 75 and 25 bytes, then over),
/// so a live region speaks as each is crossed and not at each key.
pub fn length_note(bytes: usize) -> Option<&'static str> {
    let left = MAX_CAST_BYTES as i64 - i64::try_from(bytes).unwrap_or(i64::MAX);
    match left {
        _ if bytes <= SHORT_CAST_BYTES => None,
        ALERT_LEFT.. => Some("A long cast: up to 1,024 bytes."),
        NUMBER_LEFT..ALERT_LEFT => Some("Under 75 bytes left."),
        0..NUMBER_LEFT => Some("Under 25 bytes left."),
        _ => Some("Too long to post."),
    }
}

/// How much of a screen's foot the keyboard covers, in CSS pixels, from its
/// layout viewport's height (`innerHeight`) and its visual viewport's
/// height and top (`visualViewport`): what lies below the visual viewport.
/// Under a pixel, nothing.
pub fn keyboard_inset(layout_height: f64, visual_height: f64, visual_top: f64) -> f64 {
    let covered = layout_height - visual_height - visual_top;
    if covered >= 1.0 { covered.round() } else { 0.0 }
}

/// How much of its text a reply's composer keeps of the cast it answers:
/// more than the lines it shows hold.
pub const PARENT_TEXT_CHARS: usize = 600;
/// How many of that cast's images and videos it shows.
pub const PARENT_MEDIA: usize = 4;
/// How much of that cast a screen reader hears with the reply's text box.
pub const REPLYING_CHARS: usize = 120;

/// What a reply's composer shows of the cast it answers, as the Farcaster
/// app does: who wrote it and when, the start of its text, and its first
/// images and videos. It is drawn from the cast on the page, so nothing is
/// asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentPreview {
    /// The author's display name, or username.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pfp_url: Option<String>,
    /// Unix seconds.
    pub ts: i64,
    /// The start of its text ([`PARENT_TEXT_CHARS`]), as written.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<ParentMedia>,
}

/// One of the images and videos a reply's composer shows of the cast it
/// answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentMedia {
    /// An image as the page shows it (so it's the picture the browser
    /// already has), a video's manifest or file (whose poster the browser
    /// finds), or one of Allthing's stickers.
    pub url: String,
    pub kind: ParentMediaKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParentMediaKind {
    Image,
    Video,
    Sticker,
}

impl ParentPreview {
    pub fn of(cast: &Cast) -> ParentPreview {
        let media = cast
            .media()
            .filter_map(|embed| {
                let url = embed.url()?;
                Some(match embed {
                    CastEmbed::Video { .. } => ParentMedia { url: url.to_owned(), kind: ParentMediaKind::Video },
                    _ if crate::emoji::sticker_of(url).is_some() => {
                        ParentMedia { url: url.to_owned(), kind: ParentMediaKind::Sticker }
                    }
                    _ => ParentMedia { url: resized_image_url(url, ImageSize::Inline), kind: ParentMediaKind::Image },
                })
            })
            .take(PARENT_MEDIA)
            .collect();
        ParentPreview {
            name: cast.author.name().to_owned(),
            pfp_url: cast.author.pfp_url.clone(),
            ts: cast.ts,
            text: text_start(&cast.text, PARENT_TEXT_CHARS),
            media,
        }
    }
}

/// What a screen reader hears of the cast a reply answers, with the reply's
/// text box: "Replying to @alice: What should I read next?", the cast's
/// start cut to [`REPLYING_CHARS`].
pub fn replying_to(username: &str, text: &str) -> String {
    let excerpt = truncate_title(text, REPLYING_CHARS);
    if excerpt.is_empty() { format!("Replying to @{username}") } else { format!("Replying to @{username}: {excerpt}") }
}

/// Where the Farcaster app opens a cast: its author's name and its hash's
/// first eight digits, or, for an author without a username, the whole hash.
pub fn cast_url(username: &str, hash: &str) -> String {
    if username.is_empty() {
        format!("{APP}/~/conversations/{hash}")
    } else {
        format!("{APP}/{}/{}", percent_encode(username), hash.get(..10).unwrap_or(hash))
    }
}

/// Where the Farcaster app opens an account's profile: its name, or its fid
/// for an account without a username.
pub fn profile_url(username: &str, fid: u64) -> String {
    if username.is_empty() { format!("{APP}/~/profiles/{fid}") } else { format!("{APP}/{}", percent_encode(username)) }
}

/// The Farcaster app, on the web; a phone with the app opens its links there.
const APP: &str = "https://farcaster.xyz";

/// A link Allthing hands a write to: the Farcaster app's composer with a
/// draft ([`Draft::compose_url`]), a cast ([`cast_url`]) or a profile
/// ([`profile_url`]), on farcaster.xyz, and nothing else. The QR endpoint
/// draws these, as well as sign-in and key request links.
pub fn is_handoff_link(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://farcaster.xyz/") else { return false };
    if url.len() > 4096 {
        return false;
    }
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    // As `percent_encode` writes them, and the query's own `&`, `=` and `[]`.
    let encoded = |s: &str, extra: &[u8]| {
        s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.~!*'()%".contains(&b) || extra.contains(&b))
    };
    let name = |s: &str| (1..=64).contains(&s.len()) && s != "~" && encoded(s, b"");
    let hex = |s: &str, digits: std::ops::RangeInclusive<usize>| {
        s.strip_prefix("0x").is_some_and(|h| digits.contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    let parts: Vec<&str> = path.split('/').collect();
    match (parts.as_slice(), query) {
        (["~", "compose"], query) => query.is_none_or(|q| encoded(q, b"&=[]")),
        (["~", "conversations", hash], None) => hex(hash, 40..=40),
        (["~", "profiles", fid], None) => (1..=20).contains(&fid.len()) && fid.bytes().all(|b| b.is_ascii_digit()),
        ([user], None) => name(user),
        ([user, hash], None) => name(user) && hex(hash, 8..=40),
        _ => false,
    }
}

impl Draft {
    /// Something to post: text, an embed or a quote.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.embeds.is_empty() && self.quote.is_none()
    }

    /// The cast to sign: the text, with each "@name" that `fid_of` knows
    /// as a mention, its embeds, and its parent.
    pub fn new_cast(&self, fid_of: impl Fn(&str) -> Option<u64>) -> NewCast {
        let (text, mentions) = split_mentions(self.text.trim(), fid_of);
        let mut embeds: Vec<Embed> = self.embeds.iter().map(|url| Embed::Url(url.clone())).collect();
        embeds.extend(self.quote.as_ref().and_then(|q| CastId::new(q.fid, &q.hash)).map(Embed::Cast));
        let parent = match (&self.parent_hash, self.parent_fid) {
            (Some(hash), Some(fid)) => CastId::new(fid, hash).map(Parent::Cast),
            _ => self.channel_url.clone().map(Parent::Url),
        };
        NewCast { text, mentions, embeds, parent }
    }

    /// The compose link that opens this draft in the Farcaster app.
    pub fn compose_url(&self) -> String {
        let mut query = Vec::new();
        let text = self.text.trim();
        if !text.is_empty() {
            query.push(format!("text={}", percent_encode(text)));
        }
        let quote = self.quote.as_ref().map(Quoted::url);
        for embed in self.embeds.iter().chain(quote.as_ref()).take(2) {
            query.push(format!("embeds[]={}", percent_encode(embed)));
        }
        match (&self.parent_hash, &self.channel) {
            (Some(hash), _) => query.push(format!("parentCastHash={}", percent_encode(hash))),
            (None, Some(channel)) => query.push(format!("channelKey={}", percent_encode(channel))),
            (None, None) => {}
        }
        if query.is_empty() { COMPOSE_URL.to_owned() } else { format!("{COMPOSE_URL}?{}", query.join("&")) }
    }
}

/// KLIPY's trending GIFs, or its results for `query`, page `page` (from 1).
pub fn gifs_url(key: &str, query: &str, page: u32) -> String {
    let base = format!("{KLIPY_API}/{}/gifs", percent_encode(key));
    // Animated WebP for the tiles, GIF for the cast; the strictest content
    // filter, for a public timeline.
    let common = format!("page={}&per_page={GIFS_PER_PAGE}&content_filter=high&format_filter=gif,webp", page.max(1));
    match query.trim() {
        "" => format!("{base}/trending?{common}"),
        q => format!("{base}/search?q={}&{common}", percent_encode(q)),
    }
}

/// A page of KLIPY's results: `{"result": true, "data": {"data": [...],
/// "current_page": 1, "per_page": 24, "has_next": true}}`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GifPage {
    #[serde(default)]
    pub result: bool,
    #[serde(default)]
    pub data: GifPageData,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct GifPageData {
    #[serde(default)]
    pub data: Vec<GifItem>,
    #[serde(default)]
    pub has_next: bool,
}

/// One result. Only GIFs are asked for; an ad (`"type": "ad"`), which
/// KLIPY sends only to keys it's turned on for, has no `file`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GifItem {
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub file: Option<GifFiles>,
}

/// Each size KLIPY renders a GIF at, largest first.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GifFiles {
    #[serde(default)]
    pub hd: Option<GifFormats>,
    #[serde(default)]
    pub md: Option<GifFormats>,
    #[serde(default)]
    pub sm: Option<GifFormats>,
    #[serde(default)]
    pub xs: Option<GifFormats>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct GifFormats {
    #[serde(default)]
    pub gif: Option<Rendition>,
    #[serde(default)]
    pub webp: Option<Rendition>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Rendition {
    pub url: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

/// A GIF as the picker shows it and a cast embeds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gif {
    pub slug: String,
    /// What it shows, for its label and alt text.
    pub title: String,
    /// The picker's tile and the draft's preview: the small animated WebP
    /// (`sm`, about 200 px wide and 120 KB), sharp in a grid of three on a
    /// high-density screen, where the `xs` one (90 px) blurs.
    pub tile: Rendition,
    /// What the cast embeds: the medium GIF, which every Farcaster client
    /// plays, or the nearest size whose link fits an embed.
    pub embed: Rendition,
}

impl GifItem {
    /// The GIF to show and embed, if KLIPY sent one.
    pub fn gif(&self) -> Option<Gif> {
        let file = self.file.as_ref()?;
        let sizes = [&file.md, &file.sm, &file.hd, &file.xs];
        let fits =
            |r: &&Rendition| !r.url.is_empty() && r.url.len() <= MAX_EMBED_BYTES && r.url.starts_with("https://");
        let embed = sizes.iter().filter_map(|s| s.as_ref()?.gif.as_ref()).find(fits)?.clone();
        let tiles = [&file.sm, &file.xs, &file.md];
        let tile = tiles
            .iter()
            .filter_map(|s| s.as_ref().and_then(|f| f.webp.as_ref().or(f.gif.as_ref())))
            .find(|r| r.url.starts_with("https://"))?
            .clone();
        let title = self.title.trim();
        Some(Gif {
            slug: self.slug.clone(),
            title: if title.is_empty() { "GIF".to_owned() } else { title.to_owned() },
            tile,
            embed,
        })
    }
}

impl GifPage {
    /// The page's GIFs, in KLIPY's order, and whether there are more.
    pub fn gifs(&self) -> (Vec<Gif>, bool) {
        (self.data.data.iter().filter_map(GifItem::gif).collect(), self.data.has_next)
    }
}

/// A draft and where its caret was (offsets in UTF-16 units).
pub type Edited = (String, (usize, usize));

/// The composer's undo and redo: the drafts before each change, newest
/// last, and those undone. Typing within a second of other typing is
/// undone with it, as in a text box.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edits {
    past: Vec<Edited>,
    future: Vec<Edited>,
    /// When typing last changed the draft (milliseconds), or 0.
    typed_at: f64,
}

/// How many changes can be undone.
const UNDOS: usize = 100;

impl Edits {
    /// The draft changed from `before`, by typing or not, at `now`.
    pub fn changed(&mut self, before: Edited, typed: bool, now: f64) {
        let joins = typed && self.typed_at > 0.0 && now - self.typed_at < 1000.0 && !self.past.is_empty();
        if !joins {
            self.past.push(before);
            if self.past.len() > UNDOS {
                self.past.remove(0);
            }
        }
        self.typed_at = if typed { now } else { 0.0 };
        self.future.clear();
    }

    /// The draft to go back to, from `now`.
    pub fn undo(&mut self, now: Edited) -> Option<Edited> {
        let back = self.past.pop()?;
        self.future.push(now);
        self.typed_at = 0.0;
        Some(back)
    }

    /// The draft undone last, again, from `now`.
    pub fn redo(&mut self, now: Edited) -> Option<Edited> {
        let next = self.future.pop()?;
        self.past.push(now);
        self.typed_at = 0.0;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_undo_typing_together_and_redo() {
        let at = |text: &str| (text.to_owned(), (text.len(), text.len()));
        let mut edits = Edits::default();
        assert_eq!(edits.undo(at("")), None);
        // "gm", typed within a second: one undo.
        edits.changed(at(""), true, 1000.0);
        edits.changed(at("g"), true, 1500.0);
        // A pause, then " fren": another.
        edits.changed(at("gm"), true, 4000.0);
        // A paste is a change of its own.
        edits.changed(at("gm fren"), false, 4200.0);
        assert_eq!(edits.undo(at("gm fren :pepe_gm:")), Some(at("gm fren")));
        assert_eq!(edits.undo(at("gm fren")), Some(at("gm")));
        assert_eq!(edits.undo(at("gm")), Some(at("")));
        assert_eq!(edits.undo(at("")), None);
        assert_eq!(edits.redo(at("")), Some(at("gm")));
        // A change after undoing forgets what was undone.
        edits.changed(at("gm"), false, 9000.0);
        assert_eq!(edits.redo(at("gm!")), None);
    }

    #[test]
    fn a_draft_opens_in_the_farcaster_app() {
        let draft = Draft { text: "gm & hello? 100%".into(), ..Draft::default() };
        assert_eq!(draft.compose_url(), "https://farcaster.xyz/~/compose?text=gm%20%26%20hello%3F%20100%25");
        let gif = "https://static.klipy.com/ii/abc/14/af/cat.gif";
        let reply = Draft {
            text: "  so true\n".into(),
            embeds: vec![gif.into()],
            parent_hash: Some("0x09455067393562d3296bcbc2ec1c2d6bba8ac1f1".into()),
            channel: Some("farcaster".into()),
            ..Draft::default()
        };
        assert_eq!(
            reply.compose_url(),
            "https://farcaster.xyz/~/compose?text=so%20true&embeds[]=https%3A%2F%2Fstatic.klipy.com%2Fii%2Fabc%2F14%2Faf%2Fcat.gif&parentCastHash=0x09455067393562d3296bcbc2ec1c2d6bba8ac1f1",
            "a reply goes where its parent is, without a channel of its own"
        );
        let in_channel = Draft { embeds: vec![gif.into()], channel: Some("dev".into()), ..Draft::default() };
        assert!(in_channel.compose_url().ends_with("&channelKey=dev"));
        assert!(in_channel.compose_url().starts_with("https://farcaster.xyz/~/compose?embeds[]="));
        assert_eq!(Draft::default().compose_url(), COMPOSE_URL);
        assert!(Draft::default().is_empty() && Draft { text: " \n".into(), ..Draft::default() }.is_empty());
        assert!(!in_channel.is_empty());
        let three = Draft {
            embeds: vec!["https://a.example/1".into(), "https://a.example/2".into(), "https://a.example/3".into()],
            ..Draft::default()
        };
        assert_eq!(three.compose_url().matches("embeds[]=").count(), 2);
    }

    #[test]
    fn klipy_is_asked_for_trending_or_a_search() {
        assert_eq!(
            gifs_url("k3y", "", 1),
            "https://api.klipy.com/api/v1/k3y/gifs/trending?page=1&per_page=24&content_filter=high&format_filter=gif,webp"
        );
        assert_eq!(
            gifs_url("k3y", " good morning/☕ ", 2),
            "https://api.klipy.com/api/v1/k3y/gifs/search?q=good%20morning%2F%E2%98%95&page=2&per_page=24&content_filter=high&format_filter=gif,webp"
        );
        assert!(gifs_url("a/b", "", 0).starts_with("https://api.klipy.com/api/v1/a%2Fb/gifs/trending?page=1&"));
        assert_eq!(gif_key_path(), "/api/v1/gifs/key");
    }

    const PAGE: &str = r#"{"result":true,"data":{"data":[
        {"id":1,"slug":"cat-1","title":" Cat Typing ","type":"gif","tags":[],"blur_preview":"data:image/jpeg;base64,AAAA",
         "file":{"hd":{"gif":{"url":"https://static.klipy.com/ii/a/hd.gif","width":498,"height":498,"size":4001918}},
                 "md":{"gif":{"url":"https://static.klipy.com/ii/a/md.gif","width":320,"height":320,"size":1405000},
                       "webp":{"url":"https://static.klipy.com/ii/a/md.webp","width":320,"height":320,"size":636000}},
                 "xs":{"gif":{"url":"https://static.klipy.com/ii/a/xs.gif","width":90,"height":90,"size":64000},
                       "webp":{"url":"https://static.klipy.com/ii/a/xs.webp","width":90,"height":90,"size":35000}}}},
        {"id":2,"slug":"ad-1","title":"","type":"ad"},
        {"id":3,"slug":"dog","title":"","type":"gif",
         "file":{"sm":{"gif":{"url":"https://static1.klipy.com/ii/b/sm.gif","width":200,"height":150,"size":206000},
                       "webp":{"url":"https://static1.klipy.com/ii/b/sm.webp","width":200,"height":150,"size":117000}}}}
    ],"current_page":1,"per_page":24,"has_next":true}}"#;

    #[test]
    fn results_keep_klipys_order_with_a_tile_and_an_embed_each() {
        let page: GifPage = serde_json::from_str(PAGE).unwrap();
        assert!(page.result);
        let (gifs, more) = page.gifs();
        assert!(more);
        assert_eq!(gifs.iter().map(|g| g.slug.as_str()).collect::<Vec<_>>(), ["cat-1", "dog"]);
        let cat = &gifs[0];
        assert_eq!(cat.title, "Cat Typing");
        // No `sm` size: the tile is the `xs` one.
        assert_eq!(cat.tile, Rendition { url: "https://static.klipy.com/ii/a/xs.webp".into(), width: 90, height: 90 });
        assert_eq!(cat.embed.url, "https://static.klipy.com/ii/a/md.gif");
        // Only a small size: its tile and its embed come from it.
        let dog = &gifs[1];
        assert_eq!(
            (dog.title.as_str(), dog.tile.url.as_str(), dog.embed.url.as_str()),
            ("GIF", "https://static1.klipy.com/ii/b/sm.webp", "https://static1.klipy.com/ii/b/sm.gif")
        );
    }

    #[test]
    fn an_embed_link_too_long_for_a_cast_falls_to_the_next_size() {
        let long = format!("https://static.klipy.com/{}.gif", "x".repeat(300));
        let item = GifItem {
            slug: "s".into(),
            title: "t".into(),
            file: Some(GifFiles {
                md: Some(GifFormats { gif: Some(Rendition { url: long, width: 1, height: 1 }), webp: None }),
                sm: Some(GifFormats {
                    gif: Some(Rendition { url: "https://static.klipy.com/s.gif".into(), width: 1, height: 1 }),
                    webp: Some(Rendition { url: "https://static.klipy.com/s.webp".into(), width: 1, height: 1 }),
                }),
                ..GifFiles::default()
            }),
        };
        assert_eq!(item.gif().unwrap().embed.url, "https://static.klipy.com/s.gif");
        // No GIF that fits: nothing to attach.
        let none = GifItem { file: Some(GifFiles::default()), ..item };
        assert!(none.gif().is_none());
    }

    #[test]
    fn an_empty_or_odd_answer_is_no_results() {
        for body in [r#"{}"#, r#"{"result":false}"#, r#"{"result":true,"data":{"data":[]}}"#] {
            let page: GifPage = serde_json::from_str(body).unwrap();
            assert_eq!(page.gifs(), (Vec::new(), false));
        }
    }

    #[test]
    fn a_draft_becomes_the_cast_to_sign() {
        let gif = "https://static.klipy.com/ii/abc/14/af/cat.gif";
        let hash = "0x09455067393562d3296bcbc2ec1c2d6bba8ac1f1";
        let quote = Quoted { fid: 3, hash: hash.into(), username: "dwr".into() };
        let fids = |name: &str| (name == "dwr").then_some(3);
        // A reply, quoting a cast, mentioning @dwr, with a GIF.
        let reply = Draft {
            text: "  yes @dwr, this \n".into(),
            embeds: vec![gif.into()],
            quote: Some(quote.clone()),
            parent_hash: Some("0x0102030405060708090a0b0c0d0e0f1011121314".into()),
            parent_fid: Some(42),
            channel: Some("farcaster".into()),
            channel_url: Some("https://farcaster.xyz/~/channel/farcaster".into()),
        };
        assert_eq!(
            reply.new_cast(fids),
            NewCast {
                text: "yes , this".into(),
                mentions: vec![(3, 4)],
                embeds: vec![Embed::Url(gif.into()), Embed::Cast(CastId::new(3, hash).unwrap())],
                parent: Some(Parent::Cast(CastId::new(42, "0x0102030405060708090a0b0c0d0e0f1011121314").unwrap())),
            },
            "a reply goes where its parent is, not to a channel"
        );
        // A cast in a channel, quoting alone; in the Farcaster app, the
        // quote is its link.
        let in_channel = Draft {
            quote: Some(quote.clone()),
            channel: Some("dev".into()),
            channel_url: Some("chain://eip155:1/erc721:0xabc".into()),
            ..Draft::default()
        };
        assert!(!in_channel.is_empty());
        let cast = in_channel.new_cast(fids);
        assert_eq!(cast.parent, Some(Parent::Url("chain://eip155:1/erc721:0xabc".into())));
        assert_eq!(cast.embeds, vec![Embed::Cast(CastId::new(3, hash).unwrap())]);
        assert_eq!(cast.check(), Ok(()));
        assert_eq!(quote.url(), "https://farcaster.xyz/dwr/0x09455067");
        assert_eq!(
            in_channel.compose_url(),
            "https://farcaster.xyz/~/compose?embeds[]=https%3A%2F%2Ffarcaster.xyz%2Fdwr%2F0x09455067&channelKey=dev"
        );
        // Unknown names stay as written.
        let plain = Draft { text: "hi @nobody".into(), ..Draft::default() }.new_cast(fids);
        assert_eq!((plain.text.as_str(), plain.mentions.len(), plain.parent), ("hi @nobody", 0, None));
    }

    #[test]
    fn casts_and_profiles_open_in_the_farcaster_app() {
        let hash = "0x0945506739352d78a4fd9c5c7c6f2f4b1d1a7e59";
        assert_eq!(cast_url("dwr", hash), "https://farcaster.xyz/dwr/0x09455067");
        assert_eq!(cast_url("0xael.eth", hash), "https://farcaster.xyz/0xael.eth/0x09455067");
        // An author without a username: the whole hash.
        assert_eq!(cast_url("", hash), format!("https://farcaster.xyz/~/conversations/{hash}"));
        assert_eq!(profile_url("dwr", 3), "https://farcaster.xyz/dwr");
        assert_eq!(profile_url("", 1234), "https://farcaster.xyz/~/profiles/1234");
        // A Pepe reaction, as the reply the app opens ready to post.
        let react = Draft { text: ":pepe_no:".into(), parent_hash: Some(hash.into()), ..Draft::default() };
        assert_eq!(
            react.compose_url(),
            format!("https://farcaster.xyz/~/compose?text=%3Apepe_no%3A&parentCastHash={hash}")
        );
    }

    #[test]
    fn only_the_farcaster_apps_own_links_are_handed_off() {
        let hash = "0x0945506739352d78a4fd9c5c7c6f2f4b1d1a7e59";
        let draft = Draft {
            text: "gm & hello? 100% (it's \"so\" true!)".into(),
            embeds: vec!["https://static.klipy.com/ii/abc/14/af/cat.gif".into()],
            parent_hash: Some(hash.into()),
            ..Draft::default()
        };
        for url in [
            draft.compose_url(),
            COMPOSE_URL.to_owned(),
            Draft { channel: Some("dev".into()), text: "hi".into(), ..Draft::default() }.compose_url(),
            cast_url("dwr", hash),
            cast_url("0xael.eth", hash),
            cast_url("", hash),
            profile_url("dwr", 3),
            profile_url("", 3),
        ] {
            assert!(is_handoff_link(&url), "{url}");
        }
        for url in [
            "http://farcaster.xyz/dwr",
            "https://farcaster.xyz.evil.example/dwr",
            "https://evil.example/https://farcaster.xyz/dwr",
            "https://farcaster.xyz/",
            "https://farcaster.xyz/~",
            "https://farcaster.xyz/~/settings",
            "https://farcaster.xyz/~/compose/x",
            "https://farcaster.xyz/~/compose?text=<script>",
            "https://farcaster.xyz/~/compose?text=a b",
            "https://farcaster.xyz/dwr?next=https://evil.example",
            "https://farcaster.xyz/dwr/0x0945",
            "https://farcaster.xyz/dwr/0x09455067zz",
            "https://farcaster.xyz/dwr/0x09455067/likes",
            "https://farcaster.xyz/~/conversations/0x09455067",
            "https://farcaster.xyz/~/profiles/three",
            "https://farcaster.xyz/miniapps/abc/name",
            "https://farcaster.xyz/d w r",
            "farcaster://signed-key-request?token=0x12",
        ] {
            assert!(!is_handoff_link(url), "{url}");
        }
        assert!(!is_handoff_link(&format!("https://farcaster.xyz/~/compose?text={}", "a".repeat(4100))));
    }

    fn parent(text: &str, embeds: Vec<CastEmbed>) -> Cast {
        Cast {
            hash: "0x01".into(),
            author: crate::model::UserSummary {
                fid: 42,
                username: "alice".into(),
                display_name: "Alice".into(),
                pfp_url: Some("https://example.com/alice.png".into()),
            },
            text: text.into(),
            mentions: vec![],
            ts: 1_792_235_681,
            parent_hash: None,
            parent_url: None,
            channel: None,
            embeds,
            likes: 0,
            recasts: 0,
            replies: 0,
            mini_app: false,
            spam: false,
        }
    }

    #[test]
    fn a_replys_composer_shows_who_and_when_the_start_and_the_first_four_pictures() {
        let image = |id: &str| CastEmbed::Image { url: format!("https://imagedelivery.net/acct/{id}/original") };
        let sticker = crate::emoji::STICKERS[0].embed();
        let cast = parent(
            &format!("  {}\nend  ", "word ".repeat(200)),
            vec![
                CastEmbed::Link {
                    url: "https://example.com".into(),
                    image_url: Some("https://example.com/og.png".into()),
                    title: None,
                },
                image("a"),
                CastEmbed::Cast { hash: "0x02".into(), fid: Some(7) },
                CastEmbed::Video { url: "https://stream.example/v/manifest.m3u8".into() },
                CastEmbed::Image { url: sticker.clone() },
                image("b"),
                image("c"),
            ],
        );
        let preview = ParentPreview::of(&cast);
        assert_eq!(preview.name, "Alice");
        assert_eq!(preview.pfp_url.as_deref(), Some("https://example.com/alice.png"));
        assert_eq!(preview.ts, 1_792_235_681);
        assert!(preview.text.starts_with("word word") && preview.text.ends_with("word…"), "{}", preview.text);
        assert!(preview.text.chars().count() <= PARENT_TEXT_CHARS + 1);
        // Links and quoted casts aren't pictures; images are as the page
        // shows them, and the fifth goes.
        let kinds: Vec<(&str, ParentMediaKind)> = preview.media.iter().map(|m| (m.url.as_str(), m.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("https://imagedelivery.net/acct/a/fit=scale-down,f=auto,w=720", ParentMediaKind::Image),
                ("https://stream.example/v/manifest.m3u8", ParentMediaKind::Video),
                (sticker.as_str(), ParentMediaKind::Sticker),
                ("https://imagedelivery.net/acct/b/fit=scale-down,f=auto,w=720", ParentMediaKind::Image),
            ]
        );
        // An author with no display name goes by their username; a short
        // text is as written, lines and all.
        let mut plain = parent("two\nlines", vec![]);
        plain.author.display_name.clear();
        let preview = ParentPreview::of(&plain);
        assert_eq!((preview.name.as_str(), preview.text.as_str(), preview.media.len()), ("alice", "two\nlines", 0));
    }

    #[test]
    fn the_keyboard_covers_what_lies_below_the_visual_viewport() {
        // No keyboard: the two viewports are one.
        assert_eq!(keyboard_inset(844.0, 844.0, 0.0), 0.0);
        // A keyboard 336 px tall, the page not scrolled under it.
        assert_eq!(keyboard_inset(844.0, 508.0, 0.0), 336.0);
        // Scrolled 120 px by the browser to show the focused field.
        assert_eq!(keyboard_inset(844.0, 508.0, 120.0), 216.0);
        // Rounding, and zoom's fractions, never a negative.
        assert_eq!(keyboard_inset(844.0, 843.6, 0.0), 0.0);
        assert_eq!(keyboard_inset(844.0, 507.4, 0.0), 337.0);
        assert_eq!(keyboard_inset(844.0, 600.0, 300.0), 0.0);
    }

    #[test]
    fn the_ring_fills_toward_a_cast_then_a_long_cast_and_then_counts_down() {
        assert_eq!(length(0), Length::Ring { full: 0.0, alert: false });
        assert_eq!(length(160), Length::Ring { full: 0.5, alert: false });
        // At 320 a cast is long, and the ring starts again toward 1,024.
        assert_eq!(length(320), Length::Ring { full: 320.0 / 1024.0, alert: false });
        assert_eq!(length(512), Length::Ring { full: 0.5, alert: false });
        assert_eq!(length(949), Length::Ring { full: 949.0 / 1024.0, alert: false });
        assert_eq!(length(950), Length::Ring { full: 950.0 / 1024.0, alert: true });
        assert_eq!(length(999), Length::Ring { full: 999.0 / 1024.0, alert: true });
        assert_eq!(length(1000), Length::Left(24));
        assert_eq!(length(1024), Length::Left(0));
        assert_eq!(length(1030), Length::Left(-6));
    }

    #[test]
    fn a_screen_reader_hears_the_length_only_as_it_crosses_a_line() {
        let notes: Vec<Option<&str>> =
            [0, 320, 321, 949, 950, 999, 1000, 1024, 1025].into_iter().map(length_note).collect();
        assert_eq!(
            notes,
            vec![
                None,
                None,
                Some("A long cast: up to 1,024 bytes."),
                Some("A long cast: up to 1,024 bytes."),
                Some("Under 75 bytes left."),
                Some("Under 75 bytes left."),
                Some("Under 25 bytes left."),
                Some("Under 25 bytes left."),
                Some("Too long to post."),
            ]
        );
    }

    #[test]
    fn a_screen_reader_hears_whom_a_reply_answers_and_the_start_of_what_they_said() {
        assert_eq!(replying_to("alice", "What should I\nread next?"), "Replying to @alice: What should I read next?");
        assert_eq!(replying_to("bob", "  "), "Replying to @bob");
        let long = replying_to("carol", &"word ".repeat(60));
        assert!(long.starts_with("Replying to @carol: word word") && long.ends_with('…'), "{long}");
        assert!(long.chars().count() <= "Replying to @carol: ".len() + REPLYING_CHARS + 1);
    }
}

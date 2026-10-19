// SPDX-License-Identifier: AGPL-3.0-or-later
//! Composing a cast. Allthing doesn't post: a draft (its text, a GIF, the
//! channel it goes to or the cast it answers) opens in the Farcaster app,
//! ready to post, through farcaster.xyz's compose link.
//!
//! GIFs come from KLIPY (<https://docs.klipy.com>). KLIPY asks that its API
//! and its media be reached from the reader's browser, never through a
//! partner's server, and that results keep the order and composition it
//! returns them in. The browser therefore asks KLIPY itself, with the app
//! key the Worker hands a signed-in session ([`gif_key_path`]), and shows
//! every result in order.

use serde::{Deserialize, Serialize};

use crate::api::API_PREFIX;
use crate::params::percent_encode;

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

/// A cast to post, as the Farcaster app opens it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    /// Media and links to embed: a cast takes two.
    pub embeds: Vec<String>,
    /// The cast it replies to.
    pub parent_hash: Option<String>,
    /// The channel it goes to (an id, such as "farcaster"). A reply goes
    /// where its parent is.
    pub channel: Option<String>,
}

impl Draft {
    /// Something to post: text or an embed.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.embeds.is_empty()
    }

    /// The compose link that opens this draft in the Farcaster app.
    pub fn compose_url(&self) -> String {
        let mut query = Vec::new();
        let text = self.text.trim();
        if !text.is_empty() {
            query.push(format!("text={}", percent_encode(text)));
        }
        for embed in self.embeds.iter().take(2) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

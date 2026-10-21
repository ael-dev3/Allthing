// SPDX-License-Identifier: AGPL-3.0-or-later
//! Classifying embedded URLs and picking image sizes.

use crate::model::Embed;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Link,
}

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "svg"];
const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mov", "m3u8"];
const IMAGE_HOSTS: &[&str] = &["imagedelivery.net", "i.imgur.com", "media.tenor.com"];
const VIDEO_HOSTS: &[&str] = &["stream.warpcast.com", "stream.farcaster.xyz"];

/// The lowercase host of an http(s) URL, without userinfo or port.
pub fn host_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };
    let valid = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    valid.then(|| host.to_ascii_lowercase())
}

/// Whether ".ext" appears followed by the end of the URL, "?" or "#".
fn has_extension(url: &str, exts: &[&str]) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.match_indices('.').any(|(dot, _)| {
        let after = &lower[dot + 1..];
        exts.iter()
            .any(|ext| after.starts_with(ext) && matches!(after.as_bytes().get(ext.len()), None | Some(b'?' | b'#')))
    })
}

/// Decide how to show an embedded URL, using a content type when the provider has one.
pub fn classify_url(url: &str, content_type: Option<&str>) -> MediaKind {
    if let Some(ct) = content_type {
        if ct.starts_with("image/") {
            return MediaKind::Image;
        }
        if ct.starts_with("video/") || ct.eq_ignore_ascii_case("application/x-mpegurl") {
            return MediaKind::Video;
        }
    }
    let host = host_of(url);
    let on = |hosts: &[&str]| host.as_deref().is_some_and(|h| hosts.contains(&h));
    if has_extension(url, VIDEO_EXTS) || on(VIDEO_HOSTS) {
        MediaKind::Video
    } else if has_extension(url, IMAGE_EXTS) || on(IMAGE_HOSTS) {
        MediaKind::Image
    } else {
        MediaKind::Link
    }
}

/// Whether a video URL is an HLS playlist, played through Allthing's HLS
/// player, rather than a file the browser can play by itself.
pub fn is_hls(url: &str) -> bool {
    has_extension(url, &["m3u8"]) || host_of(url).is_some_and(|h| VIDEO_HOSTS.contains(&h.as_str()))
}

/// Whether an image or video embed is a GIF, which Allthing shows at about
/// the size chat apps give one rather than as wide as a photo. It is one
/// when:
/// - its host serves nothing but GIFs ([`is_gif_host`]), whatever the
///   format (Tenor and GIPHY also send a GIF as WebP or MP4);
/// - its path ends in `.gif`, in any case (not a `.gif` in its query, or
///   in a folder's name);
/// - or its provider says it is `image/gif`. Hypersnap sends no embed
///   metadata, so a GIF uploaded to Cloudflare Images (an `imagedelivery.net`
///   address with no extension) can't be told from a photo, and stays one.
pub fn is_gif(url: &str, content_type: Option<&str>) -> bool {
    let says_gif =
        content_type.and_then(|ct| ct.split(';').next()).is_some_and(|ct| ct.trim().eq_ignore_ascii_case("image/gif"));
    let Some(host) = host_of(url) else { return says_gif };
    says_gif || is_gif_host(&host) || url_path(url).to_ascii_lowercase().ends_with(".gif")
}

/// The media hosts of GIF services, which serve only GIFs:
/// - Tenor: `media.tenor.com`, and the `media1.tenor.com` and `c.tenor.com`
///   of older links;
/// - KLIPY, whose GIFs the composer embeds: `static.klipy.com`, and
///   numbered ones such as `static1.klipy.com`;
/// - GIPHY: `media.giphy.com`, `media0.giphy.com` to `media4.giphy.com`, and
///   `i.giphy.com`.
///
/// Only exact media hosts: the services' own sites and APIs (`tenor.com`,
/// `giphy.com`, `api.klipy.com`) serve pages and JSON, and photo hosts
/// (Cloudflare Images, Imgur) serve GIFs among photos, which only a `.gif`
/// path tells apart.
fn is_gif_host(host: &str) -> bool {
    // "media", or "media" and a number ("media1").
    let named = |sub: &str, name: &str| sub.strip_prefix(name).is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()));
    if let Some(sub) = host.strip_suffix(".tenor.com") {
        named(sub, "media") || sub == "c"
    } else if let Some(sub) = host.strip_suffix(".klipy.com") {
        named(sub, "static")
    } else if let Some(sub) = host.strip_suffix(".giphy.com") {
        named(sub, "media") || sub == "i"
    } else {
        false
    }
}

/// The path of a URL, without its query or fragment: "/a/b.gif" for
/// "https://example.com/a/b.gif?w=1#top".
fn url_path(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = &rest[rest.find(['/', '?', '#']).unwrap_or(rest.len())..];
    rest.split(['?', '#']).next().unwrap_or_default()
}

/// Whether a URL opens a Farcaster mini app or frame, which Allthing does not
/// run: `farcaster.xyz/miniapps/…`, `farcaster.xyz/~/mini-apps…`,
/// `warpcast.com/~/frames…` and similar, and `*.neynar.app` mini app hosts.
pub fn is_mini_app_url(url: &str) -> bool {
    let Some(host) = host_of(url) else { return false };
    let path = url.split_once("://").and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..])).unwrap_or("/");
    let path = path.to_ascii_lowercase();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| path.starts_with(p));
    match host.strip_prefix("www.").unwrap_or(&host) {
        "farcaster.xyz" | "warpcast.com" => {
            starts(&["/miniapps/", "/~/mini-apps", "/~/miniapps", "/~/frames", "/~/frame", "/~/mini-app"])
        }
        h => h.ends_with(".neynar.app"),
    }
}

/// Sites (and their subdomains) whose pages are never mini apps, so their
/// links aren't fetched to look for mini app meta tags. Mini apps on
/// Farcaster's own hosts are found by [`is_mini_app_url`]. Block explorers'
/// pages are large and slow to answer a Worker: a Basescan link held cold
/// profile builds for the whole 2.5 s link-page timeout.
const NOT_MINI_APP_HOSTS: &[&str] = &[
    "farcaster.xyz",
    "warpcast.com",
    "youtube.com",
    "youtu.be",
    "x.com",
    "twitter.com",
    "github.com",
    "instagram.com",
    "tiktok.com",
    "reddit.com",
    "open.spotify.com",
    "vimeo.com",
    "en.wikipedia.org",
    "etherscan.io",
    "basescan.org",
    "arbiscan.io",
    "polygonscan.com",
    "celoscan.io",
    "bscscan.com",
    "uniscan.xyz",
    "blockscout.com",
    "solscan.io",
    "explorer.solana.com",
];

/// A link URL as a key for its mini app check: without its `#fragment`.
pub fn link_key(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

/// Whether a link embed's page may be a mini app, and so is worth fetching to
/// read its meta tags: an http(s) page that isn't media, isn't already known
/// to be a mini app by its URL, and isn't on a site that never serves them.
pub fn worth_checking_for_mini_app(url: &str) -> bool {
    let Some(host) = host_of(url) else { return false };
    let skipped = NOT_MINI_APP_HOSTS
        .iter()
        .any(|site| host == *site || host.strip_suffix(site).is_some_and(|sub| sub.ends_with('.')));
    classify_url(url, None) == MediaKind::Link && !is_mini_app_url(url) && !skipped
}

/// Whether the start of a page read so far reaches the end of its `<head>`,
/// where any mini app meta tags have all been: reading can stop there. Only
/// the bytes from `from` on (what arrived since the last look) are searched,
/// with the few before them that a `</head` could straddle.
pub fn head_ended(page: &[u8], from: usize) -> bool {
    const END: &[u8] = b"</head";
    let start = from.min(page.len()).saturating_sub(END.len() - 1);
    page[start..].windows(END.len()).any(|w| w.eq_ignore_ascii_case(END))
}

/// Whether a page's HTML declares a Farcaster mini app or frame in its
/// `<head>`: an `fc:miniapp` or `fc:frame` meta tag, the older `fc:frame:*`
/// tags, or an Open Frame's `of:*` tags.
pub fn html_declares_mini_app(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let mut rest = lower.as_str();
    let mut templates = 0usize;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if let Some(comment) = rest.strip_prefix("!--") {
            rest = comment.split_once("-->").map_or("", |(_, after)| after);
            continue;
        }
        let end = tag_end(rest);
        let tag = &rest[..end];
        rest = rest.get(end + 1..).unwrap_or("");
        let (closing, tag) = tag.strip_prefix('/').map_or((false, tag), |tag| (true, tag));
        let name_end = tag.find(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, ':' | '-')).unwrap_or(tag.len());
        let name = &tag[..name_end];
        if closing {
            if name == "head" && templates == 0 {
                break;
            }
            if name == "template" {
                templates = templates.saturating_sub(1);
            }
            continue;
        }
        if name == "body" && templates == 0 {
            break;
        }
        if matches!(name, "script" | "style" | "title" | "textarea" | "xmp" | "iframe" | "noembed" | "noframes") {
            // HTML treats their contents as text, even when it looks like tags.
            let close = format!("</{name}");
            rest = rest
                .match_indices(&close)
                .find_map(|(at, _)| {
                    rest.as_bytes()
                        .get(at + close.len())
                        .filter(|c| c.is_ascii_whitespace() || matches!(c, b'>' | b'/'))
                        .map(|_| &rest[at..])
                })
                .unwrap_or("");
        } else if name == "template" {
            templates += 1;
        } else if name == "meta" && templates == 0 {
            let attrs = &tag[name_end..];
            if ["name", "property"].iter().filter_map(|attr| attribute(attrs, attr)).any(|value| {
                let value = value.trim();
                value == "fc:miniapp"
                    || value == "fc:frame"
                    || value.starts_with("fc:frame:")
                    || value.starts_with("of:")
            }) {
                return true;
            }
        }
    }
    false
}

/// The tag's end, preserving '>' inside quoted attribute values. A capped
/// head can end inside a tag; its complete attributes are still useful.
fn tag_end(tag: &str) -> usize {
    let mut quote = None;
    for (at, c) in tag.char_indices() {
        if quote == Some(c) {
            quote = None;
        } else if quote.is_none() {
            match c {
                '\'' | '"' => quote = Some(c),
                '>' => return at,
                _ => {}
            }
        }
    }
    tag.len()
}

/// The value of `name` in a tag's attributes (`name="v"`, `name='v'` or
/// `name=v`), if present.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if rest.is_empty() {
            return None;
        }
        let end = rest.find(|c: char| c.is_ascii_whitespace() || c == '=').unwrap_or(rest.len());
        let key = &rest[..end];
        rest = rest[end..].trim_start();
        let Some(value) = rest.strip_prefix('=') else {
            if key == name {
                return Some("");
            }
            continue;
        };
        let value = value.trim_start();
        let (value, after) = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let value = &value[1..];
                let end = value.find(quote).unwrap_or(value.len());
                (&value[..end], value.get(end + 1..).unwrap_or(""))
            }
            _ => {
                let end = value.find(|c: char| c.is_ascii_whitespace()).unwrap_or(value.len());
                (&value[..end], &value[end..])
            }
        };
        if key == name {
            return Some(value);
        }
        rest = after;
    }
}

/// "i.imgur.com" for a URL; the raw string if it can't be parsed.
pub fn embed_domain(url: &str) -> String {
    match host_of(url) {
        Some(host) => host.strip_prefix("www.").map(str::to_owned).unwrap_or(host),
        None => url.to_owned(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageSize {
    /// 64px square, for profile pictures next to names.
    Avatar,
    /// 140px square crop.
    Thumb,
    /// Inline in a feed or a post, up to 720px wide.
    Inline,
    /// The hover preview, up to 1200px.
    Preview,
    /// The lightbox, up to 2400px.
    Full,
}

impl ImageSize {
    fn variant(self) -> &'static str {
        match self {
            ImageSize::Avatar => "anim=false,fit=cover,f=auto,w=64,h=64",
            ImageSize::Thumb => "anim=false,fit=cover,f=auto,w=140,h=140",
            ImageSize::Inline => "fit=scale-down,f=auto,w=720",
            ImageSize::Preview => "fit=scale-down,f=auto,w=1200",
            ImageSize::Full => "fit=scale-down,f=auto,w=2400",
        }
    }
}

/// Cloudflare Images (used by most Farcaster clients) resizes on the fly, which
/// turns a ~800 KB original into a few KB. Other hosts are returned unchanged.
pub fn resized_image_url(url: &str, size: ImageSize) -> String {
    const PREFIX: &str = "https://imagedelivery.net/";
    let Some(path) = url.strip_prefix(PREFIX) else {
        return url.to_owned();
    };
    let parts: Vec<&str> = path.split('/').collect();
    let plain = |s: &str| !s.is_empty() && !s.contains(['?', '#']);
    if parts.len() == 3 && parts.iter().all(|p| plain(p)) {
        format!("{PREFIX}{}/{}/{}", parts[0], parts[1], size.variant())
    } else {
        url.to_owned()
    }
}

/// What a cast with no text of its own shows in a short list (a replies
/// preview), where its images show as thumbnails: its first other embed, as
/// a few words. `None` when it has only images, or nothing.
pub fn embed_label(embeds: &[Embed]) -> Option<String> {
    embeds.iter().find_map(|embed| match embed {
        Embed::Image { .. } => None,
        Embed::Video { .. } => Some("Video".to_owned()),
        Embed::Link { url, title, .. } => Some(
            title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .or_else(|| host_of(url).map(|host| host.trim_start_matches("www.").to_owned()))
                .unwrap_or_else(|| "Link".to_owned()),
        ),
        Embed::Cast { .. } => Some("Quotes a cast".to_owned()),
    })
}

/// Embeds worth rendering under a cast's text: media and quotes always,
/// links only when the text doesn't already show the URL.
pub fn visible_embeds<'a>(embeds: &'a [Embed], text: &str) -> Vec<&'a Embed> {
    embeds
        .iter()
        .filter(|embed| match embed {
            Embed::Link { url, .. } => !text.contains(url.as_str()),
            _ => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_mini_app_meta_tags() {
        let page = |head: &str| format!("<!doctype html><html><head><title>x</title>{head}</head><body></body></html>");
        assert!(html_declares_mini_app(&page(r#"<meta name="fc:miniapp" content='{"version":"1"}'>"#)));
        assert!(html_declares_mini_app(&page(r#"<meta name="fc:frame" content="{}" />"#)));
        assert!(html_declares_mini_app(&page(r#"<META PROPERTY="fc:frame:image" CONTENT="https://x/y.png">"#)));
        assert!(html_declares_mini_app(&page(r#"<meta property='fc:frame' content=vNext>"#)));
        assert!(html_declares_mini_app(&page(r#"<meta content="1.0" property="of:version">"#)));
        assert!(html_declares_mini_app(&page(r#"<meta property=of:accepts:xmtp content=2024-02-01>"#)));
        // Ordinary pages, and the tags only in the body or in other attributes.
        assert!(!html_declares_mini_app(&page(r#"<meta property="og:title" content="fc:frame">"#)));
        assert!(!html_declares_mini_app(&page(r#"<meta name="description" content="a frame">"#)));
        assert!(!html_declares_mini_app(r#"<html><head></head><body><meta name="fc:frame"></body></html>"#));
        assert!(!html_declares_mini_app(&page(r#"<meta data-name="fc:frame" name="viewport">"#)));
        assert!(!html_declares_mini_app(""));
        // Cut off before the head ends (a capped download): what's there counts.
        assert!(html_declares_mini_app(r#"<html><head><meta name="fc:miniapp" content="{}">"#));
    }

    #[test]
    fn metadata_examples_in_comments_and_templates_are_not_declarations() {
        assert!(!html_declares_mini_app("<head><!-- <meta name='fc:frame'> --></head>"));
        assert!(!html_declares_mini_app("<head><template><meta name='fc:frame'></template></head>"));
        assert!(!html_declares_mini_app("<head><metadata name='fc:frame'></head>"));
        assert!(html_declares_mini_app("<head><!-- </head><meta name='description'> --><meta name='fc:frame'></head>"));
    }

    #[test]
    fn raw_text_elements_do_not_declare_metadata() {
        for tag in ["script", "style", "title", "textarea"] {
            let example = format!("<head><{tag}>const example = `<meta name='fc:frame'>`;</{tag}></head>");
            assert!(!html_declares_mini_app(&example));
        }
        assert!(html_declares_mini_app("<head><script>const s = '</head>';</script><meta name='fc:frame'></head>"));
        assert!(!html_declares_mini_app("<head><script>'</scripture><meta name=fc:frame>'</script></head>"));
    }

    #[test]
    fn fallback_html_examples_do_not_declare_metadata() {
        for tag in ["noframes", "iframe", "noembed", "xmp"] {
            let example = format!("<head><{tag}><meta name='fc:frame'></{tag}></head>");
            assert!(!html_declares_mini_app(&example), "metadata is text inside {tag}");
        }
        // An inert example does not hide a real declaration after it, and
        // a longer tag name in the example cannot end the raw-text element.
        assert!(html_declares_mini_app(
            "<head><noframes><meta name='description'></noframes><meta name='fc:frame'></head>"
        ));
        assert!(!html_declares_mini_app("<head><noframes></noframes-example><meta name='fc:frame'></noframes></head>"));
    }

    #[test]
    fn attribute_values_are_not_separate_attributes_or_tag_boundaries() {
        assert!(!html_declares_mini_app(r#"<head><meta content="example name='fc:frame'" name='description'></head>"#));
        assert!(!html_declares_mini_app(
            r#"<head><meta name='description' content="example property='fc:frame'"></head>"#
        ));
        assert!(!html_declares_mini_app(r#"<head><meta name name='fc:frame'></head>"#));
        assert!(html_declares_mini_app(r#"<head><meta content="a > b" name="fc:frame"></head>"#));
        assert!(!html_declares_mini_app(r#"<head><meta content="<meta name='fc:frame'>"></head>"#));
    }

    #[test]
    fn a_page_head_ends_at_its_closing_tag_even_across_chunks() {
        let page = b"<html><head><meta name=\"fc:miniapp\" content=\"{}\"></HEAD><body>";
        assert!(head_ended(page, 0));
        let split = page.iter().position(|&b| b == b'H').unwrap() + 2;
        // The first chunk ends inside the tag; the next look starts there.
        assert!(!head_ended(&page[..split], 0));
        assert!(head_ended(page, split));
        assert!(!head_ended(b"<html><head><title>x</title>", 0));
        assert!(!head_ended(b"", 0));
        assert!(!head_ended(b"</hea", 99));
    }

    #[test]
    fn a_textless_cast_is_named_by_its_first_embed_that_isnt_an_image() {
        let image = Embed::Image { url: "https://i.imgur.com/a.png".into() };
        let link = |title: Option<&str>| Embed::Link {
            url: "https://www.example.com/post".into(),
            image_url: None,
            title: title.map(str::to_owned),
        };
        assert_eq!(embed_label(std::slice::from_ref(&image)), None);
        assert_eq!(embed_label(&[]), None);
        assert_eq!(
            embed_label(&[image.clone(), Embed::Video { url: "https://s/v.m3u8".into() }]).as_deref(),
            Some("Video")
        );
        assert_eq!(embed_label(&[link(Some(" A post "))]).as_deref(), Some("A post"));
        assert_eq!(embed_label(&[link(Some("  "))]).as_deref(), Some("example.com"));
        assert_eq!(embed_label(&[Embed::Cast { hash: "0x01".into(), fid: None }]).as_deref(), Some("Quotes a cast"));
    }

    #[test]
    fn picks_links_worth_checking_for_mini_apps() {
        assert!(worth_checking_for_mini_app("https://gig-bot-2-0.vercel.app/"));
        assert!(worth_checking_for_mini_app("https://basefriend.tech/app?ref=1"));
        assert!(!worth_checking_for_mini_app("https://www.youtube.com/watch?v=x"));
        assert!(!worth_checking_for_mini_app("https://x.com/dwr/status/1"));
        assert!(!worth_checking_for_mini_app("https://farcaster.xyz/dwr/0xabc"));
        assert!(!worth_checking_for_mini_app("https://docs.farcaster.xyz/learn/what-is-farcaster"));
        assert!(!worth_checking_for_mini_app("https://m.youtube.com/watch?v=x"));
        assert!(worth_checking_for_mini_app("https://notfarcaster.xyz/"));
        assert!(!worth_checking_for_mini_app("https://basescan.org/tx/0x16fa"));
        assert!(!worth_checking_for_mini_app("https://optimistic.etherscan.io/address/0x76d0"));
        assert!(!worth_checking_for_mini_app("https://solscan.io/tx/5abc"));
        assert!(worth_checking_for_mini_app("https://notbasescan.org/"));
        assert!(!worth_checking_for_mini_app("https://farcaster.xyz/miniapps/Z6gEN6rYltaz/swarmhood"));
        assert!(!worth_checking_for_mini_app("https://i.imgur.com/a.png"));
        assert!(!worth_checking_for_mini_app("https://example.com/clip.mp4"));
        assert!(!worth_checking_for_mini_app("ipfs://bafy"));
        assert_eq!(link_key("https://a.app/x?y=1#top"), "https://a.app/x?y=1");
    }

    #[test]
    fn classifies_urls() {
        assert_eq!(classify_url("https://imagedelivery.net/acct/id/original", None), MediaKind::Image);
        assert_eq!(classify_url("https://example.com/cat.JPG?w=1", None), MediaKind::Image);
        assert_eq!(classify_url("https://stream.warpcast.com/v1/video/abc.m3u8", None), MediaKind::Video);
        assert_eq!(classify_url("https://stream.farcaster.xyz/v1/video/abc", None), MediaKind::Video);
        assert!(is_hls("https://stream.farcaster.xyz/v1/video/abc.m3u8"));
        assert!(is_hls("https://stream.warpcast.com/v1/video/abc"));
        assert!(is_hls("https://cdn.example.com/a/index.m3u8?token=1"));
        assert!(!is_hls("https://cdn.example.com/clip.mp4"));
        assert!(!is_hls("https://cdn.example.com/clip.webm#t=2"));
        assert_eq!(classify_url("https://example.com/page", None), MediaKind::Link);
        assert!(is_mini_app_url("https://farcaster.xyz/miniapps/Z6gEN6rYltaz/swarmhood?ref=3353201"));
        assert!(is_mini_app_url("https://farcaster.xyz/~/mini-apps/launch?domain=x"));
        assert!(is_mini_app_url("https://warpcast.com/~/frames/abc"));
        assert!(is_mini_app_url("https://warpcast.com/~/miniapps/abc"));
        assert!(is_mini_app_url("https://my-game.neynar.app/"));
        assert!(!is_mini_app_url("https://farcaster.xyz/~/vote"));
        assert!(!is_mini_app_url("https://farcaster.xyz/dwr/0xabc"));
        assert!(!is_mini_app_url("https://warpcast.com/~/channel/base"));
        assert!(!is_mini_app_url("https://neynar.app.example.com/"));
        assert_eq!(classify_url("https://example.com/page", Some("image/png")), MediaKind::Image);
        assert_eq!(classify_url("https://example.com/v", Some("application/x-mpegURL")), MediaKind::Video);
        assert_eq!(classify_url("https://example.com/a.mp4/b", None), MediaKind::Link);
        assert_eq!(classify_url("https://example.com/png", None), MediaKind::Link);
    }

    #[test]
    fn gifs_are_those_of_gif_services_and_images_whose_path_is_a_gif() {
        // Tenor's, KLIPY's and GIPHY's media, in whatever format they send.
        for url in [
            "https://media.tenor.com/7sV5mXX7KxUAAAAC/cat-dance.gif",
            "https://media.tenor.com/7sV5mXX7KxUAAAPo/cat-dance.mp4",
            "https://media.tenor.com/7sV5mXX7KxUAAAAe/cat-dance.webp",
            "https://MEDIA.TENOR.COM/7sV5mXX7KxUAAAAC/cat-dance",
            "https://media1.tenor.com/m/7sV5mXX7KxUAAAAC/cat-dance.gif",
            "https://c.tenor.com/7sV5mXX7KxUAAAAC/tenor.gif",
            // As KLIPY's results and the composer's casts have them.
            "https://static.klipy.com/ii/d6b0ce929193df3c242ac34b5654d2ce/bf/e4/Plhldnjk.gif",
            "https://static.klipy.com/ii/abc/14/af/cat.webp",
            "https://static1.klipy.com/ii/b/sm.mp4",
            "https://media.giphy.com/media/l0MYt5jPR6QX5pnqM/giphy.mp4",
            "https://media3.giphy.com/media/v1.Y2lkPTc5/l0MYt5jPR6QX5pnqM/giphy.webp?cid=79&rid=giphy.webp",
            "https://i.giphy.com/l0MYt5jPR6QX5pnqM.webp",
        ] {
            assert!(is_gif(url, None), "{url}");
        }
        // Any other image whose path ends in .gif, in any case.
        assert!(is_gif("https://i.postimg.cc/rFKpQ1TJ/htt3.gif", None));
        assert!(is_gif("https://media.firefly.land/farcaster/6076A171-B6BC-4647.GIF?w=720#top", None));
        assert!(is_gif("http://example.com:8080/a.Gif", None));
        // Or one its provider says is a GIF.
        assert!(is_gif("https://imagedelivery.net/acct/id/original", Some("image/gif")));
        assert!(is_gif("https://imagedelivery.net/acct/id/original", Some(" Image/GIF; charset=binary")));
        assert!(!is_gif("https://imagedelivery.net/acct/id/original", Some("image/png")));
    }

    #[test]
    fn photos_videos_and_pages_about_gifs_are_not_gifs() {
        for url in [
            // Photos and videos, on hosts that serve GIFs among them too.
            "https://imagedelivery.net/BXluQx4ige9GuW0Ia56BHw/1a9f0888-bf59-4e20-9660-07a0f7098c00/original",
            "https://i.imgur.com/sEIUnjY.jpeg",
            "https://media.firefly.land/farcaster/a620c6e7-69dc-435c-ad58-495f33c46f84.jpg",
            "https://stream.farcaster.xyz/v1/video/abc.m3u8",
            "https://example.com/clip.mp4",
            "https://media.firefly.land/post_m3u8/1c3fe8fc/1c3fe8fc.m3u8",
            // ".gif" elsewhere than at the end of the path.
            "https://i.imgur.com/abc.gifv",
            "https://example.com/render?format=gif&name=a.gif",
            "https://example.com/image#a.gif",
            "https://example.com/a.gif/still.png",
            "https://cdn.gif/photo.png",
            // The services' pages and APIs, and hosts that only look like theirs.
            "https://tenor.com/view/cat-dance-gif-25462910",
            "https://giphy.com/gifs/cat-l0MYt5jPR6QX5pnqM",
            "https://api.klipy.com/api/v1/key/gifs/trending",
            "https://klipy.com/gifs/cat",
            "https://mediax.tenor.com/a.png",
            "https://media.tenor.com.example.com/a.png",
            "https://static.klipy.co/a.png",
            "https://evilklipy.com/static/a.png",
            "https://static-1.klipy.com/a.png",
            // Not a web address.
            "ipfs://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi/a.gif",
            "not a url.gif",
        ] {
            assert!(!is_gif(url, None), "{url}");
        }
        assert!(!is_gif("https://example.com/photo.jpg", Some("image/jpeg")));
        assert!(!is_gif("https://example.com/clip", Some("video/mp4")));
        assert!(!is_gif("https://example.com/image", Some("image/gifs")));
    }

    #[test]
    fn parses_hosts() {
        assert_eq!(host_of("https://User@WWW.Example.com:8443/x?y").as_deref(), Some("www.example.com"));
        assert_eq!(host_of("ftp://example.com"), None);
        assert_eq!(host_of("not a url"), None);
        assert_eq!(embed_domain("https://www.youtube.com/watch?v=1"), "youtube.com");
        assert_eq!(embed_domain("not a url"), "not a url");
    }

    #[test]
    fn resizes_cloudflare_images_only() {
        assert_eq!(
            resized_image_url("https://imagedelivery.net/acct/abc-123/original", ImageSize::Thumb),
            "https://imagedelivery.net/acct/abc-123/anim=false,fit=cover,f=auto,w=140,h=140"
        );
        assert_eq!(
            resized_image_url("https://imagedelivery.net/acct/abc-123/original", ImageSize::Full),
            "https://imagedelivery.net/acct/abc-123/fit=scale-down,f=auto,w=2400"
        );
        assert_eq!(resized_image_url("https://i.imgur.com/x.png", ImageSize::Thumb), "https://i.imgur.com/x.png");
        assert_eq!(
            resized_image_url("https://imagedelivery.net/acct/abc/original?x=1", ImageSize::Thumb),
            "https://imagedelivery.net/acct/abc/original?x=1"
        );
    }

    #[test]
    fn hides_links_the_text_already_shows() {
        let link = Embed::Link { url: "https://a.com/x".into(), image_url: None, title: None };
        let image = Embed::Image { url: "https://i.imgur.com/x.png".into() };
        let embeds = [link.clone(), image.clone()];
        assert_eq!(visible_embeds(&embeds, "see https://a.com/x"), vec![&image]);
        assert_eq!(visible_embeds(std::slice::from_ref(&link), "no url here"), vec![&link]);
    }
}

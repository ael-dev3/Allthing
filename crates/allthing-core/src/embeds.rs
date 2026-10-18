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
/// Farcaster's own hosts are found by [`is_mini_app_url`].
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

/// Whether a page's HTML declares a Farcaster mini app or frame in its
/// `<head>`: an `fc:miniapp` or `fc:frame` meta tag, the older `fc:frame:*`
/// tags, or an Open Frame's `of:*` tags.
pub fn html_declares_mini_app(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let head = lower.find("</head").map_or(lower.as_str(), |end| &lower[..end]);
    head.match_indices("<meta").any(|(start, _)| {
        let tag = &head[start + 5..];
        let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
        ["name", "property"].iter().filter_map(|attr| attribute(tag, attr)).any(|value| {
            let value = value.trim();
            value == "fc:miniapp" || value == "fc:frame" || value.starts_with("fc:frame:") || value.starts_with("of:")
        })
    })
}

/// The value of `name` in a tag's attributes (`name="v"`, `name='v'` or
/// `name=v`), if present.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let bytes = tag.as_bytes();
    let mut from = 0;
    while let Some(found) = tag[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        // A whole attribute name: after whitespace, followed by "=".
        if at > 0 && !bytes[at - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(value) = rest.strip_prefix('=') else { continue };
        let value = value.trim_start();
        return Some(match value.chars().next() {
            Some(quote @ ('"' | '\'')) => value[1..].split(quote).next().unwrap_or(""),
            _ => value.split(|c: char| c.is_ascii_whitespace() || c == '/').next().unwrap_or(""),
        });
    }
    None
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
    fn picks_links_worth_checking_for_mini_apps() {
        assert!(worth_checking_for_mini_app("https://gig-bot-2-0.vercel.app/"));
        assert!(worth_checking_for_mini_app("https://basefriend.tech/app?ref=1"));
        assert!(!worth_checking_for_mini_app("https://www.youtube.com/watch?v=x"));
        assert!(!worth_checking_for_mini_app("https://x.com/dwr/status/1"));
        assert!(!worth_checking_for_mini_app("https://farcaster.xyz/dwr/0xabc"));
        assert!(!worth_checking_for_mini_app("https://docs.farcaster.xyz/learn/what-is-farcaster"));
        assert!(!worth_checking_for_mini_app("https://m.youtube.com/watch?v=x"));
        assert!(worth_checking_for_mini_app("https://notfarcaster.xyz/"));
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

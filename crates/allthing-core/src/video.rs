// SPDX-License-Identifier: AGPL-3.0-or-later
//! Video links that can play inside Allthing behind a click-to-load facade:
//! nothing loads from the video site until the reader presses play, and
//! YouTube plays from its no-cookie domain.

use crate::embeds::host_of;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoLink {
    YouTube { id: String, start: Option<u32> },
    Vimeo { id: String, hash: Option<String> },
    Loom { id: String },
    Streamable { id: String },
}

fn is_youtube_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn path_and_query(url: &str) -> (&str, &str) {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let without_fragment = after_scheme.split('#').next().unwrap_or("");
    let (authority_path, query) = without_fragment.split_once('?').unwrap_or((without_fragment, ""));
    let path = authority_path.find('/').map_or("", |i| &authority_path[i..]);
    (path, query)
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// "90", "90s", "1m30s", "1h2m3s" as seconds.
fn parse_start(value: &str) -> Option<u32> {
    if let Ok(seconds) = value.parse::<u32>() {
        return Some(seconds);
    }
    let mut total = 0u32;
    let mut number = 0u32;
    let mut any = false;
    for c in value.chars() {
        if let Some(d) = c.to_digit(10) {
            number = number.checked_mul(10)?.checked_add(d)?;
            any = true;
        } else {
            let unit = match c {
                'h' => 3600,
                'm' => 60,
                's' => 1,
                _ => return None,
            };
            total = total.checked_add(number.checked_mul(unit)?)?;
            number = 0;
        }
    }
    (any && number == 0).then_some(total)
}

/// Recognize a playable video page URL.
pub fn detect(url: &str) -> Option<VideoLink> {
    let host = host_of(url)?;
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let host = host.strip_prefix("m.").unwrap_or(host);
    let (path, query) = path_and_query(url);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match host {
        "youtube.com" | "music.youtube.com" | "youtube-nocookie.com" | "youtu.be" => {
            let id = if host == "youtu.be" {
                segments.first().copied()
            } else {
                match segments.as_slice() {
                    ["watch", ..] => query_param(query, "v"),
                    ["shorts" | "embed" | "live" | "v", id, ..] => Some(*id),
                    _ => None,
                }
            }?;
            let start = query_param(query, "t").or_else(|| query_param(query, "start")).and_then(parse_start);
            is_youtube_id(id).then(|| VideoLink::YouTube { id: id.to_owned(), start })
        }
        "vimeo.com" | "player.vimeo.com" => {
            let numeric = |s: &&str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            let index = segments.iter().position(numeric)?;
            let hash = segments
                .get(index + 1)
                .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()) && !h.is_empty())
                .map(|h| (*h).to_owned())
                .or_else(|| query_param(query, "h").map(str::to_owned));
            Some(VideoLink::Vimeo { id: segments[index].to_owned(), hash })
        }
        "loom.com" => match segments.as_slice() {
            ["share" | "embed", id, ..] if id.len() >= 16 && id.bytes().all(|b| b.is_ascii_alphanumeric()) => {
                Some(VideoLink::Loom { id: (*id).to_owned() })
            }
            _ => None,
        },
        "streamable.com" => {
            let id = match segments.as_slice() {
                ["e" | "o", id, ..] => *id,
                [id] => *id,
                _ => return None,
            };
            (id.len() >= 4 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
                .then(|| VideoLink::Streamable { id: id.to_owned() })
        }
        _ => None,
    }
}

impl VideoLink {
    pub fn provider(&self) -> &'static str {
        match self {
            VideoLink::YouTube { .. } => "YouTube",
            VideoLink::Vimeo { .. } => "Vimeo",
            VideoLink::Loom { .. } => "Loom",
            VideoLink::Streamable { .. } => "Streamable",
        }
    }

    /// The player to load once the reader presses play.
    pub fn player_url(&self) -> String {
        match self {
            VideoLink::YouTube { id, start } => {
                let start = start.map(|s| format!("&start={s}")).unwrap_or_default();
                format!("https://www.youtube-nocookie.com/embed/{id}?autoplay=1&rel=0&playsinline=1{start}")
            }
            VideoLink::Vimeo { id, hash } => {
                let hash = hash.as_ref().map(|h| format!("&h={h}")).unwrap_or_default();
                format!("https://player.vimeo.com/video/{id}?autoplay=1&dnt=1{hash}")
            }
            VideoLink::Loom { id } => format!("https://www.loom.com/embed/{id}?autoplay=1"),
            VideoLink::Streamable { id } => format!("https://streamable.com/e/{id}?autoplay=1"),
        }
    }

    /// A poster image, where the provider has a static, keyless URL for one.
    pub fn thumbnail_url(&self) -> Option<String> {
        match self {
            VideoLink::YouTube { id, .. } => Some(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")),
            _ => None,
        }
    }

    /// Stable key for deduplicating the same video linked twice.
    pub fn key(&self) -> String {
        match self {
            VideoLink::YouTube { id, .. } => format!("youtube:{id}"),
            VideoLink::Vimeo { id, .. } => format!("vimeo:{id}"),
            VideoLink::Loom { id } => format!("loom:{id}"),
            VideoLink::Streamable { id } => format!("streamable:{id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yt(id: &str, start: Option<u32>) -> Option<VideoLink> {
        Some(VideoLink::YouTube { id: id.into(), start })
    }

    #[test]
    fn detects_youtube_forms() {
        assert_eq!(detect("https://youtu.be/cBT6vU55Gkg?si=aAB2OcQ0urYNil8N"), yt("cBT6vU55Gkg", None));
        assert_eq!(detect("https://www.youtube.com/watch?v=Z2WxWg4BMrQ"), yt("Z2WxWg4BMrQ", None));
        assert_eq!(
            detect("https://m.youtube.com/watch?feature=share&v=Z2WxWg4BMrQ&t=1m30s"),
            yt("Z2WxWg4BMrQ", Some(90))
        );
        assert_eq!(detect("https://youtube.com/shorts/-LKOqk6zpW0"), yt("-LKOqk6zpW0", None));
        assert_eq!(detect("https://music.youtube.com/watch?v=Z2WxWg4BMrQ"), yt("Z2WxWg4BMrQ", None));
        assert_eq!(detect("https://youtu.be/Z2WxWg4BMrQ?t=42"), yt("Z2WxWg4BMrQ", Some(42)));
        assert_eq!(detect("https://www.youtube.com/@channel"), None);
        assert_eq!(detect("https://youtu.be/short"), None);
    }

    #[test]
    fn detects_other_providers() {
        assert_eq!(detect("https://vimeo.com/76979871"), Some(VideoLink::Vimeo { id: "76979871".into(), hash: None }));
        assert_eq!(
            detect("https://vimeo.com/76979871/abc123def"),
            Some(VideoLink::Vimeo { id: "76979871".into(), hash: Some("abc123def".into()) })
        );
        assert_eq!(
            detect("https://www.loom.com/share/0281766fa2d04bb788eaf19e65135184?sid=x"),
            Some(VideoLink::Loom { id: "0281766fa2d04bb788eaf19e65135184".into() })
        );
        assert_eq!(detect("https://streamable.com/moo"), None);
        assert_eq!(detect("https://streamable.com/abcd12"), Some(VideoLink::Streamable { id: "abcd12".into() }));
        assert_eq!(detect("https://example.com/watch?v=Z2WxWg4BMrQ"), None);
    }

    #[test]
    fn builds_privacy_friendly_players() {
        let link = detect("https://youtu.be/Z2WxWg4BMrQ?t=42").unwrap();
        assert_eq!(
            link.player_url(),
            "https://www.youtube-nocookie.com/embed/Z2WxWg4BMrQ?autoplay=1&rel=0&playsinline=1&start=42"
        );
        assert_eq!(link.thumbnail_url().as_deref(), Some("https://i.ytimg.com/vi/Z2WxWg4BMrQ/hqdefault.jpg"));
        let vimeo = detect("https://vimeo.com/76979871").unwrap();
        assert_eq!(vimeo.player_url(), "https://player.vimeo.com/video/76979871?autoplay=1&dnt=1");
        assert_eq!(vimeo.thumbnail_url(), None);
        assert_eq!(parse_start("1h2m3s"), Some(3723));
        assert_eq!(parse_start("x"), None);
    }
}

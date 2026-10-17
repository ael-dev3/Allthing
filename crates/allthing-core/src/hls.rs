// SPDX-License-Identifier: AGPL-3.0-or-later
//! Just enough HLS for Farcaster videos: master and media playlists of VOD
//! streams with fMP4 (CMAF) segments, as Cloudflare Stream serves them. The
//! browser app feeds these segments to Media Source Extensions where the
//! browser has no native HLS.

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub uri: String,
    pub bandwidth: u64,
    pub width: u32,
    pub height: u32,
    /// e.g. ["avc1.4d401f", "mp4a.40.2"].
    pub codecs: Vec<String>,
    pub audio_group: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioRendition {
    pub group_id: String,
    pub uri: Option<String>,
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MasterPlaylist {
    pub variants: Vec<Variant>,
    pub audio: Vec<AudioRendition>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub uri: String,
    pub duration: f64,
    /// Presentation start time, the sum of earlier durations.
    pub start: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaPlaylist {
    pub init_uri: Option<String>,
    pub segments: Vec<Segment>,
    pub ended: bool,
}

impl MediaPlaylist {
    pub fn duration(&self) -> f64 {
        self.segments.last().map_or(0.0, |s| s.start + s.duration)
    }

    /// Index of the segment playing at `time` (clamped to the stream).
    pub fn segment_at(&self, time: f64) -> usize {
        let index = self.segments.partition_point(|s| s.start + s.duration <= time);
        index.min(self.segments.len().saturating_sub(1))
    }
}

/// Split an attribute list (`KEY=VALUE,KEY="quoted, value"`).
pub fn attributes(list: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = list.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_owned();
        rest = &rest[eq + 1..];
        let value;
        if let Some(stripped) = rest.strip_prefix('"') {
            let end = stripped.find('"').unwrap_or(stripped.len());
            value = stripped[..end].to_owned();
            rest = stripped.get(end + 1..).unwrap_or("");
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            value = rest[..end].trim().to_owned();
            rest = &rest[end..];
        }
        rest = rest.trim_start().strip_prefix(',').unwrap_or(rest).trim_start();
        out.push((key, value));
    }
    out
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty())
}

/// `None` unless the text is an HLS master playlist with at least one variant.
pub fn parse_master(text: &str) -> Option<MasterPlaylist> {
    let mut lines = lines(text).peekable();
    if lines.next()? != "#EXTM3U" {
        return None;
    }
    let mut master = MasterPlaylist::default();
    while let Some(line) = lines.next() {
        if let Some(list) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let attrs = attributes(list);
            let Some(uri) = lines.next_if(|l| !l.starts_with('#')) else { continue };
            let (width, height) = attr(&attrs, "RESOLUTION")
                .and_then(|r| r.split_once('x'))
                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                .unwrap_or((0, 0));
            master.variants.push(Variant {
                uri: uri.to_owned(),
                bandwidth: attr(&attrs, "BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0),
                width,
                height,
                codecs: attr(&attrs, "CODECS")
                    .map(|c| c.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect())
                    .unwrap_or_default(),
                audio_group: attr(&attrs, "AUDIO").map(str::to_owned),
            });
        } else if let Some(list) = line.strip_prefix("#EXT-X-MEDIA:") {
            let attrs = attributes(list);
            if attr(&attrs, "TYPE") == Some("AUDIO") {
                master.audio.push(AudioRendition {
                    group_id: attr(&attrs, "GROUP-ID").unwrap_or_default().to_owned(),
                    uri: attr(&attrs, "URI").map(str::to_owned),
                    default: attr(&attrs, "DEFAULT") == Some("YES"),
                });
            }
        }
    }
    (!master.variants.is_empty()).then_some(master)
}

/// `None` unless the text is an HLS media playlist with at least one segment.
pub fn parse_media(text: &str) -> Option<MediaPlaylist> {
    let mut lines = lines(text);
    if lines.next()? != "#EXTM3U" {
        return None;
    }
    let mut playlist = MediaPlaylist::default();
    let mut pending: Option<f64> = None;
    let mut start = 0.0;
    for line in lines {
        if let Some(list) = line.strip_prefix("#EXT-X-MAP:") {
            playlist.init_uri = attr(&attributes(list), "URI").map(str::to_owned);
        } else if let Some(info) = line.strip_prefix("#EXTINF:") {
            pending = info.split(',').next().and_then(|d| d.trim().parse().ok());
        } else if line == "#EXT-X-ENDLIST" {
            playlist.ended = true;
        } else if !line.starts_with('#') {
            let duration = pending.take().unwrap_or(0.0);
            playlist.segments.push(Segment { uri: line.to_owned(), duration, start });
            start += duration;
        }
    }
    (!playlist.segments.is_empty()).then_some(playlist)
}

/// The smallest variant at least `target_height` tall, else the tallest.
pub fn choose_variant(master: &MasterPlaylist, target_height: u32) -> Option<&Variant> {
    let mut variants: Vec<&Variant> = master.variants.iter().collect();
    variants.sort_by_key(|v| (v.height, v.bandwidth));
    variants.iter().find(|v| v.height >= target_height).or(variants.last()).copied()
}

/// The audio rendition URI for a variant, if its audio is a separate stream.
pub fn audio_uri<'a>(master: &'a MasterPlaylist, variant: &Variant) -> Option<&'a str> {
    let group = variant.audio_group.as_deref()?;
    let in_group = || master.audio.iter().filter(|a| a.group_id == group && a.uri.is_some());
    in_group().find(|a| a.default).or_else(|| in_group().next()).and_then(|a| a.uri.as_deref())
}

/// Split a variant's codecs into (video, audio) MIME types for MSE.
pub fn mime_types(codecs: &[String]) -> (Option<String>, Option<String>) {
    let is_audio = |c: &str| {
        c.starts_with("mp4a")
            || c.starts_with("opus")
            || c.starts_with("ac-3")
            || c.starts_with("ec-3")
            || c.starts_with("flac")
    };
    let video = codecs.iter().find(|c| !is_audio(c)).map(|c| format!("video/mp4; codecs=\"{c}\""));
    let audio = codecs.iter().find(|c| is_audio(c)).map(|c| format!("audio/mp4; codecs=\"{c}\""));
    (video, audio)
}

/// Cloudflare Stream serves a poster next to the manifest:
/// `…/<token>/manifest/video.m3u8` → `…/<token>/thumbnails/thumbnail.jpg`.
pub fn stream_thumbnail(manifest_url: &str, height: u32) -> Option<String> {
    let base = manifest_url.split('?').next()?.strip_suffix("/manifest/video.m3u8")?;
    base.contains("cloudflarestream.com/").then(|| format!("{base}/thumbnails/thumbnail.jpg?height={height}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = r#"#EXTM3U
#EXT-X-VERSION:6
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="group_audio",NAME="original",LANGUAGE="en",DEFAULT=YES,AUTOSELECT=YES,URI="audio.m3u8"
#EXT-X-STREAM-INF:RESOLUTION=1080x1080,CODECS="avc1.4d4020,mp4a.40.2",BANDWIDTH=2183760,AUDIO="group_audio"
v1080.m3u8
#EXT-X-STREAM-INF:RESOLUTION=720x720,CODECS="avc1.4d401f,mp4a.40.2",BANDWIDTH=1116658,AUDIO="group_audio"
v720.m3u8
#EXT-X-STREAM-INF:RESOLUTION=240x240,CODECS="avc1.42c014,mp4a.40.2",BANDWIDTH=428440,AUDIO="group_audio"
v240.m3u8
"#;

    const MEDIA: &str = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:5\n\
#EXT-X-MAP:URI=\"../../id/video/720/init.mp4?p=x&s=y\"\n#EXTINF:4.00000,\n../../id/video/720/seg_1.mp4?p=a\n\
#EXTINF:4.00000,\n../../id/video/720/seg_2.mp4?p=b\n#EXTINF:0.59,\n../../id/video/720/seg_3.mp4?p=c\n#EXT-X-ENDLIST\n";

    #[test]
    fn parses_master_playlists() {
        let master = parse_master(MASTER).unwrap();
        assert_eq!(master.variants.len(), 3);
        assert_eq!(master.variants[1].height, 720);
        assert_eq!(master.variants[1].codecs, vec!["avc1.4d401f", "mp4a.40.2"]);
        assert_eq!(choose_variant(&master, 600).unwrap().uri, "v720.m3u8");
        assert_eq!(choose_variant(&master, 100).unwrap().uri, "v240.m3u8");
        assert_eq!(choose_variant(&master, 4000).unwrap().uri, "v1080.m3u8");
        assert_eq!(audio_uri(&master, &master.variants[0]), Some("audio.m3u8"));
        assert_eq!(parse_master("#EXTM3U\n#EXTINF:1,\nx.ts"), None);
        assert_eq!(parse_master("garbage"), None);
    }

    #[test]
    fn parses_media_playlists() {
        let media = parse_media(MEDIA).unwrap();
        assert_eq!(media.init_uri.as_deref(), Some("../../id/video/720/init.mp4?p=x&s=y"));
        assert_eq!(media.segments.len(), 3);
        assert!(media.ended);
        assert!((media.duration() - 8.59).abs() < 1e-9);
        assert_eq!(media.segments[2].start, 8.0);
        assert_eq!(media.segment_at(0.0), 0);
        assert_eq!(media.segment_at(4.0), 1);
        assert_eq!(media.segment_at(7.99), 1);
        assert_eq!(media.segment_at(100.0), 2);
    }

    #[test]
    fn attribute_lists_keep_quoted_commas() {
        let attrs = attributes(r#"CODECS="avc1.4d401f,mp4a.40.2",BANDWIDTH=1,NAME="a, b""#);
        assert_eq!(attrs[0], ("CODECS".into(), "avc1.4d401f,mp4a.40.2".into()));
        assert_eq!(attrs[1], ("BANDWIDTH".into(), "1".into()));
        assert_eq!(attrs[2], ("NAME".into(), "a, b".into()));
    }

    #[test]
    fn mse_types_and_thumbnails() {
        let (video, audio) = mime_types(&["avc1.4d401f".into(), "mp4a.40.2".into()]);
        assert_eq!(video.as_deref(), Some("video/mp4; codecs=\"avc1.4d401f\""));
        assert_eq!(audio.as_deref(), Some("audio/mp4; codecs=\"mp4a.40.2\""));
        assert_eq!(
            stream_thumbnail("https://customer-x.cloudflarestream.com/TOKEN/manifest/video.m3u8", 480).as_deref(),
            Some("https://customer-x.cloudflarestream.com/TOKEN/thumbnails/thumbnail.jpg?height=480")
        );
        assert_eq!(stream_thumbnail("https://example.com/manifest/video.m3u8", 480), None);
    }
}

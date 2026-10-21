// SPDX-License-Identifier: AGPL-3.0-or-later
//! A reader's attachments: the images and videos they add to a cast. A
//! signed-in reader's browser sends the file itself as the body of a
//! `POST` to [`MEDIA_PATH`], with its type and length; the Worker keeps it
//! in R2 and serves it at `/m/<fid>/<id>.<ext>` on the site's own origin
//! ([`MediaKey`]), which is the URL the cast embeds, so any Farcaster app
//! can load it.
//!
//! The Worker never reads a file's bytes: it hands the request's own body
//! to R2. So a file is only ever the type it was declared as, one of
//! [`MediaType`], and it is always served as that type with `nosniff` and a
//! sandbox policy ([`served_headers`]), so a file that isn't what it says
//! can't run as a page.
//!
//! Limits: 10 MiB an image and 50 MiB a video; per reader, 300 MiB and 60
//! files in any 24 hours; 2,000 uploads a day from everyone together, which
//! keeps R2's writes and D1's rows within their free plans; and 9 GiB
//! stored in all, under R2's free 10 GB-month ([`admit`]). Pure logic,
//! tested natively; the Worker's side is `allthing-worker/src/media.rs`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::guard::TokenBucket;

/// `GET`: whether attachments can be uploaded here, and their limits
/// ([`MediaLimits`]); 404 when this server keeps none, 401 without a
/// session. `POST`: one file, as the body ([`Uploaded`]).
pub const MEDIA_PATH: &str = "/api/v1/media";

/// Where the Worker serves attachments: `/m/<fid>/<id>.<ext>`.
pub const SERVED_PREFIX: &str = "/m/";

const MIB: u64 = 1024 * 1024;
/// The largest image, and the largest video.
pub const MAX_IMAGE_BYTES: u64 = 10 * MIB;
pub const MAX_VIDEO_BYTES: u64 = 50 * MIB;
/// The most embeds a cast carries: Farcaster's limit.
pub const MAX_EMBEDS: u32 = 2;
/// The longest URL a cast can embed: Farcaster's limit, in bytes.
pub const MAX_EMBED_URL_BYTES: usize = 256;
/// What one reader may upload in any 24 hours.
pub const READER_FILES: u32 = 60;
pub const READER_BYTES: u64 = 300 * MIB;
/// The window those count in: the past 24 hours, rolling.
pub const WINDOW: i64 = 86_400;
/// Everyone's attachments together: 9 GiB, safely under R2's free 10
/// GB-month (9.31 GiB), which no upload may take the total past.
pub const TOTAL_BYTES: u64 = 9 * 1024 * MIB;
/// Uploads a UTC day, everyone's together: under R2's free million writes
/// a month (62,000 at most), and 8,000 of D1's free 100,000 rows written a
/// day, which the indexer and the active-accounts cron share.
pub const UPLOADS_PER_DAY: u32 = 2_000;
/// Uploads a reader may start a minute, in each isolate.
pub const UPLOADS_PER_MINUTE: u32 = 20;

/// How long browsers and other apps keep a file: a year, as it never
/// changes (its name is random, and never reused).
pub const SERVED_CACHE: &str = "public, max-age=31536000, immutable";
/// The policy a file is always served with. Opened as a page, it runs
/// nothing (`sandbox`, without scripts) and loads nothing but this site's
/// images and videos: what the browser's own viewer of an image or a video
/// loads (the file itself), with its inline styles. Without `media-src`, a
/// video opened in a tab of its own wouldn't play.
pub const SERVED_CSP: &str = "default-src 'none'; img-src 'self'; media-src 'self'; style-src 'unsafe-inline'; sandbox";

/// A type of file a cast can carry, and the only types a file is kept or
/// served as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaType {
    Png,
    Jpeg,
    Gif,
    Webp,
    Mp4,
    Mov,
    Webm,
}

impl MediaType {
    pub const ALL: [MediaType; 7] = [
        MediaType::Png,
        MediaType::Jpeg,
        MediaType::Gif,
        MediaType::Webp,
        MediaType::Mp4,
        MediaType::Mov,
        MediaType::Webm,
    ];

    /// Its MIME type, as it is kept and served.
    pub fn mime(self) -> &'static str {
        match self {
            MediaType::Png => "image/png",
            MediaType::Jpeg => "image/jpeg",
            MediaType::Gif => "image/gif",
            MediaType::Webp => "image/webp",
            MediaType::Mp4 => "video/mp4",
            MediaType::Mov => "video/quicktime",
            MediaType::Webm => "video/webm",
        }
    }

    /// The extension of its name, which is how other apps tell an embed's
    /// kind before they read it.
    pub fn extension(self) -> &'static str {
        match self {
            MediaType::Png => "png",
            MediaType::Jpeg => "jpg",
            MediaType::Gif => "gif",
            MediaType::Webp => "webp",
            MediaType::Mp4 => "mp4",
            MediaType::Mov => "mov",
            MediaType::Webm => "webm",
        }
    }

    pub fn is_video(self) -> bool {
        matches!(self, MediaType::Mp4 | MediaType::Mov | MediaType::Webm)
    }

    /// The largest file of this type.
    pub fn max_bytes(self) -> u64 {
        if self.is_video() { MAX_VIDEO_BYTES } else { MAX_IMAGE_BYTES }
    }

    /// The type a `Content-Type` value names, if it is one of these: its
    /// essence, in any case, with any parameters ignored.
    pub fn from_content_type(value: &str) -> Option<MediaType> {
        let essence = value.split(';').next().unwrap_or_default().trim();
        MediaType::ALL.into_iter().find(|t| t.mime().eq_ignore_ascii_case(essence))
    }

    /// The type an extension stands for, exactly as [`MediaType::extension`]
    /// writes it.
    pub fn from_extension(extension: &str) -> Option<MediaType> {
        MediaType::ALL.into_iter().find(|t| t.extension() == extension)
    }
}

/// The type a file is served as: the one kept with it, if it is one of
/// ours, and otherwise bytes that nothing renders.
pub fn served_type(stored: Option<&str>) -> &'static str {
    stored.and_then(MediaType::from_content_type).map_or("application/octet-stream", MediaType::mime)
}

/// The headers every file is served with, beside its length and range:
/// its type, a year's caching, its ETag, `nosniff` and the sandbox policy,
/// and leave for any site or app to show it.
pub fn served_headers(content_type: &'static str, etag: &str) -> [(&'static str, String); 8] {
    [
        ("content-type", content_type.to_owned()),
        ("cache-control", SERVED_CACHE.to_owned()),
        ("accept-ranges", "bytes".to_owned()),
        ("etag", etag.to_owned()),
        ("x-content-type-options", "nosniff".to_owned()),
        ("content-security-policy", SERVED_CSP.to_owned()),
        ("cross-origin-resource-policy", "cross-origin".to_owned()),
        ("access-control-allow-origin", "*".to_owned()),
    ]
}

/// What `GET` [`MEDIA_PATH`] answers where attachments can be uploaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaLimits {
    pub max_image_bytes: u64,
    pub max_video_bytes: u64,
    pub max_embeds: u32,
}

pub const LIMITS: MediaLimits =
    MediaLimits { max_image_bytes: MAX_IMAGE_BYTES, max_video_bytes: MAX_VIDEO_BYTES, max_embeds: MAX_EMBEDS };

/// What `POST` [`MEDIA_PATH`] answers: the URL to embed, the type it is
/// served as, and its size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uploaded {
    pub url: String,
    #[serde(rename = "type")]
    pub media_type: String,
    pub bytes: u64,
}

/// Why an upload is refused: its HTTP status, what the reader is told, and
/// when to try again, in seconds, when that is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub message: &'static str,
    pub retry_after: Option<u32>,
}

const fn refused(status: u16, message: &'static str) -> Refused {
    Refused { status, message, retry_after: None }
}

pub const BAD_TYPE: Refused = refused(400, "Attach a PNG, JPEG, GIF or WebP image, or an MP4, MOV or WebM video");
pub const NO_LENGTH: Refused = refused(400, "An upload needs its size, in its Content-Length");
pub const EMPTY: Refused = refused(400, "That file is empty");
pub const IMAGE_TOO_LARGE: Refused = refused(413, "Images can be up to 10 MB");
pub const VIDEO_TOO_LARGE: Refused = refused(413, "Videos can be up to 50 MB");
pub const READER_FILES_USED: Refused =
    refused(507, "You've attached 60 files in the past 24 hours, the most a day takes");
pub const READER_BYTES_USED: Refused =
    refused(507, "That would take your uploads past 300 MB in 24 hours, the most a day takes");
pub const FULL: Refused = refused(507, "Allthing has no room for more attachments just now");
pub const DAY_FULL: Refused = refused(507, "Allthing has taken all the attachments it can today; try again tomorrow");

/// The file a `POST` declares, if it is one to keep: its type, from the
/// list, and its length, within its type's limit.
pub fn check_upload(content_type: Option<&str>, content_length: Option<&str>) -> Result<(MediaType, u64), Refused> {
    let media = content_type.and_then(MediaType::from_content_type).ok_or(BAD_TYPE)?;
    let length = content_length.map(str::trim).filter(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()));
    let length = length.and_then(|l| l.parse::<u64>().ok()).ok_or(NO_LENGTH)?;
    if length == 0 {
        return Err(EMPTY);
    }
    if length > media.max_bytes() {
        return Err(if media.is_video() { VIDEO_TOO_LARGE } else { IMAGE_TOO_LARGE });
    }
    Ok((media, length))
}

/// What a reader has uploaded in the [`WINDOW`], what everyone has today,
/// and what is stored in all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub files: u32,
    pub bytes: u64,
    /// When the oldest of the reader's files in the window came, Unix
    /// seconds: once it is a day old, it no longer counts.
    pub oldest: Option<i64>,
    /// Everyone's uploads this UTC day.
    pub today: u32,
    /// Everyone's files together.
    pub stored: u64,
}

/// The name of the row that counts a UTC day's uploads (`now`, Unix
/// seconds), beside the total's `all`: `d` and the days since 1970.
pub fn day_row(now: i64) -> String {
    format!("d{}", now.div_euclid(86_400))
}

/// Whether a file of `bytes` may be kept, at `now` (Unix seconds): within
/// the space left in all, and the reader's day.
pub fn admit(usage: &Usage, bytes: u64, now: i64) -> Result<(), Refused> {
    if usage.stored.saturating_add(bytes) > TOTAL_BYTES {
        return Err(FULL);
    }
    if usage.today >= UPLOADS_PER_DAY {
        let tomorrow = (86_400 - now.rem_euclid(86_400)) as u32;
        return Err(Refused { retry_after: Some(tomorrow), ..DAY_FULL });
    }
    // When the reader's oldest file in the window leaves it.
    let freed = usage.oldest.map(|oldest| (oldest + WINDOW - now).clamp(1, WINDOW) as u32);
    if usage.files >= READER_FILES {
        return Err(Refused { retry_after: freed, ..READER_FILES_USED });
    }
    if usage.bytes.saturating_add(bytes) > READER_BYTES {
        return Err(Refused { retry_after: freed, ..READER_BYTES_USED });
    }
    Ok(())
}

/// An attachment's place: whose it is, its random id (32 lowercase hex
/// digits) and its type. It is kept in R2 as `<fid>/<id>.<ext>`
/// ([`MediaKey::object`]) and served at `/m/` and that
/// ([`MediaKey::path`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaKey {
    pub fid: u64,
    pub id: String,
    pub media: MediaType,
}

/// The digits of a random id: 16 bytes, in hex.
const ID_DIGITS: usize = 32;

impl MediaKey {
    pub fn new(fid: u64, id: &str, media: MediaType) -> Option<MediaKey> {
        let id_ok = id.len() == ID_DIGITS && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        (fid > 0 && id_ok).then(|| MediaKey { fid, id: id.to_owned(), media })
    }

    /// Its key in R2.
    pub fn object(&self) -> String {
        format!("{}/{}.{}", self.fid, self.id, self.media.extension())
    }

    /// Where the Worker serves it.
    pub fn path(&self) -> String {
        format!("{SERVED_PREFIX}{}", self.object())
    }

    /// The URL a cast embeds: the site's own origin, then its path.
    pub fn url(&self, origin: &str) -> String {
        format!("{}{}", origin.trim_end_matches('/'), self.path())
    }

    /// The attachment a path names, if it names one exactly: `/m/`, an
    /// account's fid, `/`, a 32-digit lowercase hex id, `.` and one of our
    /// extensions. Anything else is no attachment.
    pub fn from_path(path: &str) -> Option<MediaKey> {
        let (fid, name) = path.strip_prefix(SERVED_PREFIX)?.split_once('/')?;
        let fid_ok =
            !fid.is_empty() && fid.len() <= 20 && !fid.starts_with('0') && fid.bytes().all(|b| b.is_ascii_digit());
        let (id, extension) = name.split_once('.')?;
        MediaKey::new(fid_ok.then(|| fid.parse().ok()).flatten()?, id, MediaType::from_extension(extension)?)
    }
}

/// The largest number a JavaScript number holds exactly: positions past it
/// are cut to it, as R2's range reads take numbers (and workers-rs stops on
/// a larger one), and no file comes near it.
pub const MAX_POSITION: u64 = (1 << 53) - 1;

/// The one byte range a `Range` header asks for. Anything else (another
/// unit, several ranges, a malformed one) is no range, and the whole file
/// is served, as HTTP allows (RFC 9110, 14.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByteRange {
    /// `bytes=first-last`, both inclusive.
    Span(u64, u64),
    /// `bytes=first-`: from there to the end.
    From(u64),
    /// `bytes=-n`: the last `n` bytes.
    Last(u64),
}

impl ByteRange {
    pub fn parse(header: &str) -> Option<ByteRange> {
        let (unit, spec) = header.trim().split_once('=')?;
        if !unit.trim().eq_ignore_ascii_case("bytes") || spec.contains(',') {
            return None;
        }
        let (first, last) = spec.trim().split_once('-')?;
        let number = |digits: &str| -> Option<u64> {
            let digits = digits.trim();
            let ok = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
            // Past u64, a position is past any file: as good as the largest.
            ok.then(|| digits.parse::<u64>().unwrap_or(u64::MAX).min(MAX_POSITION))
        };
        match (first.trim().is_empty(), last.trim().is_empty()) {
            (true, false) => Some(ByteRange::Last(number(last)?)),
            (false, true) => Some(ByteRange::From(number(first)?)),
            (false, false) => {
                let (first, last) = (number(first)?, number(last)?);
                (first <= last).then_some(ByteRange::Span(first, last))
            }
            (true, true) => None,
        }
    }

    /// The first and last byte it covers in a file of `size` bytes, or
    /// `None` when it covers none (416, Range Not Satisfiable).
    pub fn within(self, size: u64) -> Option<(u64, u64)> {
        let end = size.checked_sub(1)?;
        match self {
            ByteRange::Span(first, last) => (first <= end).then(|| (first, last.min(end))),
            ByteRange::From(first) => (first <= end).then_some((first, end)),
            ByteRange::Last(0) => None,
            ByteRange::Last(n) => Some((size - n.min(size), end)),
        }
    }
}

/// What R2 is asked to read for a range: from an offset, for a length or to
/// the end; or the last bytes. Every number is one JavaScript holds exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeRead {
    From { offset: u64, length: Option<u64> },
    Suffix(u64),
}

impl ByteRange {
    pub fn read(self) -> RangeRead {
        match self {
            ByteRange::Span(first, last) => {
                RangeRead::From { offset: first, length: Some((last - first).saturating_add(1).min(MAX_POSITION)) }
            }
            ByteRange::From(first) => RangeRead::From { offset: first, length: None },
            ByteRange::Last(n) => RangeRead::Suffix(n),
        }
    }
}

/// `Content-Range` for the bytes `first` to `last` of `size`.
pub fn content_range(first: u64, last: u64, size: u64) -> String {
    format!("bytes {first}-{last}/{size}")
}

/// `Content-Range` for a range that covers none of `size` bytes.
pub fn unsatisfied_range(size: u64) -> String {
    format!("bytes */{size}")
}

/// Whether an `If-None-Match` header names `etag` (with its quotes, or
/// without): `*`, or any of its tags, compared weakly (RFC 9110, 13.1.2).
pub fn none_match(header: &str, etag: &str) -> bool {
    let bare = |tag: &str| tag.trim().trim_start_matches("W/").trim_matches('"').to_owned();
    let ours = bare(etag);
    !ours.is_empty() && header.split(',').map(bare).any(|tag| tag == "*" || tag == ours)
}

/// Each reader's uploads a minute, in one isolate: a bucket of
/// [`UPLOADS_PER_MINUTE`] that refills one every 3 seconds. It forgets
/// readers whose bucket is full again once it holds many.
#[derive(Debug, Default)]
pub struct UploadLimiter {
    buckets: HashMap<u64, TokenBucket>,
}

/// How many readers' buckets the limiter holds before it forgets the
/// full ones.
const LIMITER_READERS: usize = 4_096;

impl UploadLimiter {
    /// Whether `fid` may start an upload at `now_ms`, or in how many
    /// seconds.
    pub fn allow(&mut self, fid: u64, now_ms: f64) -> Result<(), u32> {
        if self.buckets.len() >= LIMITER_READERS && !self.buckets.contains_key(&fid) {
            self.buckets.retain(|_, bucket| bucket.available(now_ms) < UPLOADS_PER_MINUTE);
            if self.buckets.len() >= LIMITER_READERS {
                self.buckets.clear();
            }
        }
        let per_second = f64::from(UPLOADS_PER_MINUTE) / 60.0;
        let bucket =
            self.buckets.entry(fid).or_insert_with(|| TokenBucket::new(UPLOADS_PER_MINUTE, per_second, now_ms));
        if bucket.try_take(now_ms) {
            return Ok(());
        }
        // In whole milliseconds first: a third of a token a second isn't
        // exact in binary, and 3,000.0000000000005 ms is 3 s.
        Err((bucket.wait_ms(now_ms).round() / 1000.0).ceil().max(1.0) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn each_type_has_its_mime_type_extension_and_limit() {
        let table: Vec<_> = MediaType::ALL.iter().map(|t| (t.mime(), t.extension(), t.max_bytes())).collect();
        assert_eq!(
            table,
            [
                ("image/png", "png", 10_485_760),
                ("image/jpeg", "jpg", 10_485_760),
                ("image/gif", "gif", 10_485_760),
                ("image/webp", "webp", 10_485_760),
                ("video/mp4", "mp4", 52_428_800),
                ("video/quicktime", "mov", 52_428_800),
                ("video/webm", "webm", 52_428_800),
            ]
        );
        for media in MediaType::ALL {
            assert_eq!(MediaType::from_content_type(media.mime()), Some(media));
            assert_eq!(MediaType::from_extension(media.extension()), Some(media));
            assert_eq!(media.is_video(), media.mime().starts_with("video/"));
            // Allthing's own feeds show it as what it is, from its URL.
            let kind =
                crate::embeds::classify_url(&format!("https://allthing.social/m/1/{ID}.{}", media.extension()), None);
            let expected =
                if media.is_video() { crate::embeds::MediaKind::Video } else { crate::embeds::MediaKind::Image };
            assert_eq!(kind, expected, "{media:?}");
        }
    }

    #[test]
    fn only_the_listed_types_are_taken() {
        assert_eq!(MediaType::from_content_type("IMAGE/PNG"), Some(MediaType::Png));
        assert_eq!(MediaType::from_content_type(" video/mp4 ; codecs=\"avc1\""), Some(MediaType::Mp4));
        for other in [
            "",
            "image/svg+xml",
            "text/html",
            "image/jpg",
            "image/heic",
            "application/octet-stream",
            "video/x-matroska",
            "image/png+xml",
            "image",
        ] {
            assert_eq!(MediaType::from_content_type(other), None, "{other}");
        }
        for other in ["jpeg", "PNG", "svg", "html", "", "mp4 "] {
            assert_eq!(MediaType::from_extension(other), None, "{other}");
        }
    }

    #[test]
    fn a_file_is_served_only_as_one_of_our_types() {
        assert_eq!(served_type(Some("image/webp")), "image/webp");
        assert_eq!(served_type(Some("video/quicktime")), "video/quicktime");
        assert_eq!(served_type(Some("text/html")), "application/octet-stream");
        assert_eq!(served_type(Some("image/svg+xml")), "application/octet-stream");
        assert_eq!(served_type(None), "application/octet-stream");
        // Always with nosniff and the sandbox, whatever it is.
        for content_type in [served_type(Some("image/png")), served_type(Some("text/html"))] {
            let headers = served_headers(content_type, "\"abc\"");
            let get = |name: &str| headers.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str());
            assert_eq!(get("content-type"), Some(content_type));
            assert_eq!(get("x-content-type-options"), Some("nosniff"));
            assert_eq!(
                get("content-security-policy"),
                Some("default-src 'none'; img-src 'self'; media-src 'self'; style-src 'unsafe-inline'; sandbox")
            );
            assert_eq!(get("cache-control"), Some("public, max-age=31536000, immutable"));
            assert_eq!(get("cross-origin-resource-policy"), Some("cross-origin"));
            assert_eq!(get("accept-ranges"), Some("bytes"));
            assert_eq!(get("etag"), Some("\"abc\""));
        }
    }

    #[test]
    fn an_upload_declares_its_type_and_length() {
        assert_eq!(check_upload(Some("image/png"), Some("1234")), Ok((MediaType::Png, 1234)));
        assert_eq!(check_upload(Some("video/webm"), Some(" 52428800 ")), Ok((MediaType::Webm, MAX_VIDEO_BYTES)));
        assert_eq!(check_upload(Some("image/gif"), Some("10485760")), Ok((MediaType::Gif, MAX_IMAGE_BYTES)));
        assert_eq!(check_upload(Some("image/gif"), Some("10485761")), Err(IMAGE_TOO_LARGE));
        assert_eq!(check_upload(Some("video/quicktime"), Some("52428801")), Err(VIDEO_TOO_LARGE));
        assert_eq!(check_upload(Some("image/jpeg"), Some("0")), Err(EMPTY));
        for length in
            [None, Some(""), Some("-1"), Some("12a"), Some("+5"), Some("1e6"), Some("99999999999999999999999")]
        {
            assert_eq!(check_upload(Some("image/jpeg"), length), Err(NO_LENGTH), "{length:?}");
        }
        // The type is checked first, so a wrong one says so whatever its size.
        assert_eq!(check_upload(Some("text/html"), Some("10")), Err(BAD_TYPE));
        assert_eq!(check_upload(None, Some("10")), Err(BAD_TYPE));
        assert_eq!(check_upload(Some("text/html"), None), Err(BAD_TYPE));
        assert_eq!((BAD_TYPE.status, NO_LENGTH.status, EMPTY.status), (400, 400, 400));
        assert_eq!((IMAGE_TOO_LARGE.status, VIDEO_TOO_LARGE.status), (413, 413));
    }

    #[test]
    fn quotas_count_the_readers_day_and_the_space_left() {
        let quiet = Usage::default();
        assert_eq!(admit(&quiet, MAX_VIDEO_BYTES, NOW), Ok(()));
        // The reader's 60th file is taken, a 61st isn't, until the oldest
        // is a day old.
        let busy = Usage { files: 59, bytes: 10 * MIB, oldest: Some(NOW - 3_600), ..quiet };
        assert_eq!(admit(&busy, MIB, NOW), Ok(()));
        let full_day = Usage { files: 60, ..busy };
        assert_eq!(admit(&full_day, 1, NOW), Err(Refused { retry_after: Some(82_800), ..READER_FILES_USED }));
        // 300 MiB a day, counting this file.
        let heavy = Usage { files: 7, bytes: 260 * MIB, oldest: Some(NOW - WINDOW + 30), ..quiet };
        assert_eq!(admit(&heavy, 40 * MIB, NOW), Ok(()));
        assert_eq!(admit(&heavy, 40 * MIB + 1, NOW), Err(Refused { retry_after: Some(30), ..READER_BYTES_USED }));
        // Without the oldest's time, there's no telling when.
        let unknown = Usage { oldest: None, ..full_day };
        assert_eq!(admit(&unknown, 1, NOW).unwrap_err().retry_after, None);
        // An oldest already out of the window says to try again at once.
        let stale = Usage { oldest: Some(NOW - 2 * WINDOW), ..full_day };
        assert_eq!(admit(&stale, 1, NOW).unwrap_err().retry_after, Some(1));
        // 9 GiB in all, whoever asks.
        let nearly = Usage { stored: TOTAL_BYTES - MIB, ..quiet };
        assert_eq!(admit(&nearly, MIB, NOW), Ok(()));
        assert_eq!(admit(&nearly, MIB + 1, NOW), Err(FULL));
        assert_eq!(admit(&Usage { stored: u64::MAX, ..quiet }, 1, NOW), Err(FULL));
        // 2,000 uploads a UTC day from everyone, until midnight.
        let busy_day = Usage { today: UPLOADS_PER_DAY - 1, ..quiet };
        assert_eq!(admit(&busy_day, 1, NOW), Ok(()));
        let day_done = Usage { today: UPLOADS_PER_DAY, ..quiet };
        let midnight = NOW - NOW % 86_400 + 86_400;
        assert_eq!(admit(&day_done, 1, NOW), Err(Refused { retry_after: Some((midnight - NOW) as u32), ..DAY_FULL }));
        assert_eq!(admit(&day_done, 1, midnight - 1).unwrap_err().retry_after, Some(1));
        assert_eq!(admit(&Usage { stored: TOTAL_BYTES, ..day_done }, 1, NOW), Err(FULL), "full says so first");
        assert_eq!(day_row(NOW), "d20833");
        assert_eq!(day_row(midnight - 1), "d20833");
        assert_eq!(day_row(midnight), "d20834");
        assert_eq!(day_row(0), "d0");
        let statuses = [READER_FILES_USED.status, READER_BYTES_USED.status, FULL.status, DAY_FULL.status];
        assert_eq!(statuses, [507; 4]);
        // Within R2's free million writes a month, and a tenth of D1's free
        // rows written a day at four rows an upload.
        const { assert!(UPLOADS_PER_DAY as u64 * 31 < 1_000_000 && UPLOADS_PER_DAY * 4 <= 10_000) };
        // Under R2's free 10 GB-month (10^10 bytes), whatever a reader does.
        const { assert!(TOTAL_BYTES < 10_000_000_000) };
    }

    #[test]
    fn keys_paths_and_urls() {
        let key = MediaKey::new(1234, ID, MediaType::Jpeg).unwrap();
        assert_eq!(key.object(), format!("1234/{ID}.jpg"));
        assert_eq!(key.path(), format!("/m/1234/{ID}.jpg"));
        assert_eq!(key.url("https://allthing.social"), format!("https://allthing.social/m/1234/{ID}.jpg"));
        assert_eq!(key.url("http://127.0.0.1:8787/"), format!("http://127.0.0.1:8787/m/1234/{ID}.jpg"));
        for media in MediaType::ALL {
            let key = MediaKey::new(42, ID, media).unwrap();
            assert_eq!(MediaKey::from_path(&key.path()), Some(key));
        }
        assert_eq!(MediaKey::new(0, ID, MediaType::Png), None, "no account");
        assert_eq!(MediaKey::new(1, &ID.to_uppercase(), MediaType::Png), None);
        assert_eq!(MediaKey::new(1, &ID[1..], MediaType::Png), None);
        assert_eq!(MediaKey::new(1, &format!("{}g", &ID[1..]), MediaType::Png), None);
    }

    #[test]
    fn an_embed_url_fits_a_cast() {
        // The longest: the largest fid and the longest extension.
        let longest = MediaKey::new(u64::MAX, ID, MediaType::Webm).unwrap().url("https://allthing.social");
        assert_eq!(longest.len(), 84);
        assert!(longest.len() <= MAX_EMBED_URL_BYTES);
        // A fork's origin has room to spare too.
        let fork =
            MediaKey::new(u64::MAX, ID, MediaType::Webp).unwrap().url(&format!("https://{}.example", "a".repeat(150)));
        assert!(fork.len() <= MAX_EMBED_URL_BYTES, "{}", fork.len());
    }

    #[test]
    fn served_paths_are_read_strictly() {
        let good = format!("/m/1234/{ID}.png");
        assert!(MediaKey::from_path(&good).is_some());
        for bad in [
            "/m/".to_owned(),
            "/m/1234".to_owned(),
            format!("/m/1234/{ID}"),
            format!("/m/1234/{ID}."),
            format!("/m/1234/{ID}.PNG"),
            format!("/m/1234/{ID}.jpeg"),
            format!("/m/1234/{ID}.svg"),
            format!("/m/1234/{ID}.png.html"),
            format!("/m/1234/{ID}.png/"),
            format!("/m/1234/x/{ID}.png"),
            format!("/m/01234/{ID}.png"),
            format!("/m/0/{ID}.png"),
            format!("/m/-1/{ID}.png"),
            format!("/m/12a/{ID}.png"),
            format!("/m/%31/{ID}.png"),
            format!("/m/123456789012345678901/{ID}.png"),
            format!("/m/1234/{}.png", ID.to_uppercase()),
            format!("/m/1234/{}.png", &ID[2..]),
            format!("/m/1234/{ID}0.png"),
            format!("/m/1234/../{ID}.png"),
            format!("/m//{ID}.png"),
            format!("/x/1234/{ID}.png"),
            format!("m/1234/{ID}.png"),
        ] {
            assert_eq!(MediaKey::from_path(&bad), None, "{bad}");
        }
        // The largest fid still is one.
        assert_eq!(MediaKey::from_path(&format!("/m/{}/{ID}.gif", u64::MAX)).map(|k| k.fid), Some(u64::MAX));
        assert_eq!(MediaKey::from_path(&format!("/m/18446744073709551616/{ID}.gif")), None, "past u64");
    }

    #[test]
    fn range_headers() {
        use ByteRange::*;
        assert_eq!(ByteRange::parse("bytes=0-499"), Some(Span(0, 499)));
        assert_eq!(ByteRange::parse("bytes=0-0"), Some(Span(0, 0)));
        assert_eq!(ByteRange::parse("bytes=0-1"), Some(Span(0, 1)), "Safari's first probe");
        assert_eq!(ByteRange::parse("bytes=500-"), Some(From(500)));
        assert_eq!(ByteRange::parse("bytes=0-"), Some(From(0)));
        assert_eq!(ByteRange::parse("bytes=-500"), Some(Last(500)));
        assert_eq!(ByteRange::parse(" Bytes = 10 - 20 "), Some(Span(10, 20)));
        // Huge positions are kept within what R2's numbers can hold.
        assert_eq!(ByteRange::parse("bytes=0-99999999999999999999999"), Some(Span(0, MAX_POSITION)));
        assert_eq!(ByteRange::parse("bytes=18446744073709551615-"), Some(From(MAX_POSITION)));
        assert_eq!(ByteRange::parse("bytes=-18446744073709551615"), Some(Last(MAX_POSITION)));
        for ignored in [
            "",
            "bytes",
            "bytes=",
            "bytes=-",
            "items=0-1",
            "bytes=5-4",
            "bytes=0-1,4-5",
            "bytes=a-b",
            "bytes=1-2-3",
            "bytes=+1-2",
        ] {
            assert_eq!(ByteRange::parse(ignored), None, "{ignored}");
        }
    }

    #[test]
    fn ranges_within_a_file() {
        use ByteRange::*;
        assert_eq!(Span(0, 499).within(1000), Some((0, 499)));
        assert_eq!(Span(500, 5000).within(1000), Some((500, 999)), "cut at the end");
        assert_eq!(Span(999, 999).within(1000), Some((999, 999)));
        assert_eq!(Span(1000, 1001).within(1000), None);
        assert_eq!(From(0).within(1000), Some((0, 999)));
        assert_eq!(From(999).within(1000), Some((999, 999)));
        assert_eq!(From(1000).within(1000), None);
        assert_eq!(Last(1).within(1000), Some((999, 999)));
        assert_eq!(Last(400).within(1000), Some((600, 999)));
        assert_eq!(Last(5000).within(1000), Some((0, 999)), "more than there is: all of it");
        assert_eq!(Last(0).within(1000), None);
        for range in [Span(0, 0), From(0), Last(1)] {
            assert_eq!(range.within(0), None, "an empty file has no bytes");
        }
        // As R2 reads them, never past JavaScript's exact numbers.
        assert_eq!(Span(10, 19).read(), RangeRead::From { offset: 10, length: Some(10) });
        assert_eq!(From(7).read(), RangeRead::From { offset: 7, length: None });
        assert_eq!(Last(7).read(), RangeRead::Suffix(7));
        let huge = ByteRange::parse("bytes=0-99999999999999999999").unwrap().read();
        assert_eq!(huge, RangeRead::From { offset: 0, length: Some(MAX_POSITION) });
        assert_eq!(content_range(0, 499, 1000), "bytes 0-499/1000");
        assert_eq!(unsatisfied_range(1000), "bytes */1000");
    }

    #[test]
    fn if_none_match() {
        let etag = "\"5d41402abc4b2a76b9719d911017c592\"";
        assert!(none_match(etag, etag));
        assert!(none_match("\"5d41402abc4b2a76b9719d911017c592\"", "5d41402abc4b2a76b9719d911017c592"));
        assert!(none_match("W/\"5d41402abc4b2a76b9719d911017c592\"", etag), "compared weakly");
        assert!(none_match("\"other\", \"5d41402abc4b2a76b9719d911017c592\"", etag));
        assert!(none_match("*", etag));
        assert!(!none_match("\"other\"", etag));
        assert!(!none_match("", etag));
        assert!(!none_match("\"\"", "\"\""), "an empty tag is no tag");
    }

    #[test]
    fn a_reader_may_start_twenty_uploads_a_minute() {
        let mut limiter = UploadLimiter::default();
        let start = 1_000_000.0;
        for _ in 0..UPLOADS_PER_MINUTE {
            assert_eq!(limiter.allow(7, start), Ok(()));
        }
        assert_eq!(limiter.allow(7, start), Err(3), "one comes back every 3 s");
        assert_eq!(limiter.allow(7, start + 1_500.0), Err(2));
        // Another reader has their own.
        assert_eq!(limiter.allow(8, start), Ok(()));
        assert_eq!(limiter.allow(7, start + 3_001.0), Ok(()));
        assert!(limiter.allow(7, start + 3_001.0).is_err());
        // A minute on, all twenty are back.
        for _ in 0..UPLOADS_PER_MINUTE {
            assert_eq!(limiter.allow(7, start + 63_002.0), Ok(()));
        }
    }

    #[test]
    fn the_limiter_forgets_readers_whose_bucket_is_full_again() {
        let mut limiter = UploadLimiter::default();
        for fid in 0..LIMITER_READERS as u64 {
            assert_eq!(limiter.allow(fid, 0.0), Ok(()));
        }
        // Reader 1 uses all theirs.
        for _ in 1..UPLOADS_PER_MINUTE {
            assert_eq!(limiter.allow(1, 0.0), Ok(()));
        }
        assert!(limiter.allow(1, 0.0).is_err());
        // Half a minute on, a new reader makes room: the buckets full again
        // go, and reader 1's, half full, stays.
        assert_eq!(limiter.allow(u64::MAX, 30_000.0), Ok(()));
        assert_eq!(limiter.buckets.len(), 2);
        assert!(limiter.buckets.contains_key(&1));
    }

    #[test]
    fn the_answers_are_as_the_composer_reads_them() {
        assert_eq!(
            serde_json::to_string(&LIMITS).unwrap(),
            r#"{"max_image_bytes":10485760,"max_video_bytes":52428800,"max_embeds":2}"#
        );
        let uploaded =
            Uploaded { url: "https://allthing.social/m/1/x.png".into(), media_type: "image/png".into(), bytes: 7 };
        let json = serde_json::to_string(&uploaded).unwrap();
        assert_eq!(json, r#"{"url":"https://allthing.social/m/1/x.png","type":"image/png","bytes":7}"#);
        assert_eq!(serde_json::from_str::<Uploaded>(&json).unwrap(), uploaded);
        assert!(MEDIA_PATH.starts_with(crate::api::API_PREFIX));
    }
}

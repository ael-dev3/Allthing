// SPDX-License-Identifier: AGPL-3.0-or-later
//! A reader's video as HLS, which is the only kind of video the Farcaster
//! app plays in a cast: it shows any `.m3u8` embed as a video, wherever it
//! is hosted, and a plain `.mp4` as "No preview found for shared link".
//!
//! The browser remuxes the MP4 or MOV the reader picked ([`remux`]): the
//! same coded samples, without a byte decoded or re-encoded, put back
//! together as one fragmented MP4 (an init segment, then a `moof` and an
//! `mdat` for each few seconds, each starting at a key frame). That file is
//! uploaded and kept as `<id>.mp4`, which any browser plays by itself, and
//! where its fragments are is sent with it as an [`HlsIndex`], a line short
//! enough for R2's custom metadata. From that the Worker serves, without
//! reading the file, a master playlist at `<id>.m3u8` ([`master_playlist`])
//! and a media playlist at `<id>.v.m3u8` ([`media_playlist`]), whose
//! segments are byte ranges of `<id>.mp4`. The cast embeds `<id>.m3u8`.
//!
//! Only what every HLS player decodes in fragmented MP4 is remuxed: H.264
//! or HEVC video, with or without AAC audio. Anything else stays the file
//! it was. The file's other tracks and its metadata (`udta`, `meta`: the
//! place a phone recorded it, its model) are left out.

use std::fmt::Write as _;

/// The most segments a video is cut into, so that its index fits in R2's
/// custom metadata (2 KiB, keys included).
pub const MAX_SEGMENTS: usize = 80;
/// The shortest a segment is, in seconds, as HLS players expect.
pub const SEGMENT_SECONDS: f64 = 4.0;
/// The longest an index is, in bytes.
pub const MAX_INDEX_BYTES: usize = 1_800;
/// The header the browser sends an index in, with the upload.
pub const INDEX_HEADER: &str = "x-allthing-hls";
/// The custom metadata key the Worker keeps it under, in R2.
pub const INDEX_METADATA: &str = "hls";
/// An HLS playlist's type.
pub const PLAYLIST_TYPE: &str = "application/vnd.apple.mpegurl";

/// Where a remuxed video's fragments are, and what the master playlist
/// says of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlsIndex {
    pub width: u32,
    pub height: u32,
    /// RFC 6381 codecs, e.g. `avc1.64001f,mp4a.40.2`.
    pub codecs: String,
    /// The init segment's length: bytes `0..init`.
    pub init: u64,
    /// Each segment's duration, in milliseconds, and length in bytes, in
    /// order, from `init` on.
    pub segments: Vec<(u32, u64)>,
}

impl HlsIndex {
    /// The whole file's length.
    pub fn total(&self) -> u64 {
        self.init + self.segments.iter().map(|(_, len)| len).sum::<u64>()
    }

    /// As one line: `1;<w>x<h>;<codecs>;<init>;<ms>:<len>,…`.
    pub fn encode(&self) -> String {
        let mut out = format!("1;{}x{};{};{};", self.width, self.height, self.codecs, self.init);
        for (i, (ms, len)) in self.segments.iter().enumerate() {
            let _ = write!(out, "{}{ms}:{len}", if i > 0 { "," } else { "" });
        }
        out
    }

    /// An index exactly as [`HlsIndex::encode`] writes it, and only one
    /// that makes sense: anything else is none.
    pub fn parse(line: &str) -> Option<HlsIndex> {
        if line.len() > MAX_INDEX_BYTES {
            return None;
        }
        let mut parts = line.split(';');
        if parts.next()? != "1" {
            return None;
        }
        let (width, height) = parts.next()?.split_once('x')?;
        let codecs = parts.next()?;
        let init = number(parts.next()?)?;
        let segments = parts
            .next()?
            .split(',')
            .map(|s| {
                let (ms, len) = s.split_once(':')?;
                let (ms, len) = (number(ms)?, number(len)?);
                (1..=3_600_000).contains(&ms).then_some(())?;
                (len > 0).then_some((ms as u32, len))
            })
            .collect::<Option<Vec<_>>>()?;
        let codecs_ok = !codecs.is_empty()
            && codecs.len() <= 64
            && codecs.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b',');
        let (width, height) = (number(width)?, number(height)?);
        let ok = parts.next().is_none()
            && codecs_ok
            && init > 0
            && (1..=MAX_SEGMENTS).contains(&segments.len())
            && (1..=16_384).contains(&width)
            && (1..=16_384).contains(&height);
        ok.then(|| HlsIndex { width: width as u32, height: height as u32, codecs: codecs.to_owned(), init, segments })
    }
}

/// Plain decimal digits, no sign or leading zero, as a number.
fn number(digits: &str) -> Option<u64> {
    let ok = !digits.is_empty()
        && digits.len() <= 15
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'));
    ok.then(|| digits.parse().ok()).flatten()
}

/// `<id>.m3u8`: the one rendition, with its size, codecs and bit rate, as
/// the Farcaster app reads them.
pub fn master_playlist(index: &HlsIndex, id: &str) -> String {
    let seconds = index.segments.iter().map(|(ms, _)| *ms as u64).sum::<u64>().max(1) as f64 / 1000.0;
    let average = (index.total() as f64 * 8.0 / seconds).ceil() as u64;
    let peak = index
        .segments
        .iter()
        .map(|(ms, len)| (*len as f64 * 8.0 / (*ms as f64 / 1000.0)).ceil() as u64)
        .max()
        .unwrap_or(average)
        .max(average);
    format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n\
         #EXT-X-STREAM-INF:BANDWIDTH={peak},AVERAGE-BANDWIDTH={average},RESOLUTION={}x{},CODECS=\"{}\"\n{id}.v.m3u8\n",
        index.width, index.height, index.codecs
    )
}

/// `<id>.v.m3u8`: the segments, as byte ranges of `<id>.mp4`.
pub fn media_playlist(index: &HlsIndex, id: &str) -> String {
    let longest = index.segments.iter().map(|(ms, _)| *ms).max().unwrap_or(1);
    let mut out = format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n\
         #EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MAP:URI=\"{id}.mp4\",BYTERANGE=\"{}@0\"\n",
        longest.div_ceil(1000),
        index.init
    );
    let mut offset = index.init;
    for (ms, len) in &index.segments {
        let _ = write!(out, "#EXTINF:{}.{:03},\n#EXT-X-BYTERANGE:{len}@{offset}\n{id}.mp4\n", ms / 1000, ms % 1000);
        offset += len;
    }
    out.push_str("#EXT-X-ENDLIST\n");
    out
}

/// A remuxed video: the fragmented MP4, and where its fragments are.
#[derive(Clone, Debug)]
pub struct Remuxed {
    pub bytes: Vec<u8>,
    pub index: HlsIndex,
}

/// Why a file wasn't remuxed: it is kept as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotRemuxed {
    /// Not an MP4 or MOV we can read.
    Unreadable,
    /// No H.264 or HEVC video track, or audio that isn't AAC.
    Unsupported,
    /// Already fragmented, or with no samples.
    Unsuitable,
}

// ---------------------------------------------------------------------------
// Reading boxes

/// The boxes in `data`: each one's type and its body.
fn boxes(data: &[u8]) -> Option<Vec<([u8; 4], &[u8])>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < data.len() {
        let rest = &data[at..];
        if rest.len() < 8 {
            return None;
        }
        let size32 = u32::from_be_bytes(rest[0..4].try_into().ok()?) as u64;
        let kind: [u8; 4] = rest[4..8].try_into().ok()?;
        let (header, size) = match size32 {
            0 => (8u64, rest.len() as u64),
            1 => (16, u64::from_be_bytes(rest.get(8..16)?.try_into().ok()?)),
            n => (8, n),
        };
        if size < header || size > rest.len() as u64 {
            return None;
        }
        out.push((kind, &rest[header as usize..size as usize]));
        at += size as usize;
    }
    Some(out)
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data)?.into_iter().find(|(k, _)| k == kind).map(|(_, body)| body)
}

fn path<'a>(data: &'a [u8], kinds: &[&[u8; 4]]) -> Option<&'a [u8]> {
    kinds.iter().try_fold(data, |at, kind| child(at, kind))
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// A table's 32-bit entries.
fn words(entries: &[u8]) -> Vec<u32> {
    let (words, _) = entries.as_chunks::<4>();
    words.iter().map(|w| u32::from_be_bytes(*w)).collect()
}

/// A full box's table: its entry count, then `count` entries of `width`
/// bytes from `skip` on (past version, flags and anything before the count).
fn table(body: &[u8], skip: usize, width: usize) -> Option<(u32, &[u8])> {
    let count = u32_at(body, skip)?;
    let start = skip + 4;
    let entries = body.get(start..start.checked_add((count as usize).checked_mul(width)?)?)?;
    Some((count, entries))
}

// ---------------------------------------------------------------------------
// Writing boxes

fn put_box(out: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(kind);
    body(out);
    let size = (out.len() - start) as u32;
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn put_full(out: &mut Vec<u8>, kind: &[u8; 4], version: u8, flags: u32, body: impl FnOnce(&mut Vec<u8>)) {
    put_box(out, kind, |out| {
        out.extend_from_slice(&((u32::from(version) << 24) | (flags & 0x00ff_ffff)).to_be_bytes());
        body(out);
    });
}

fn put_raw(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    put_box(out, kind, |out| out.extend_from_slice(body));
}

// ---------------------------------------------------------------------------
// Tracks

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Video,
    Audio,
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    offset: u64,
    size: u32,
    dts: u64,
    duration: u32,
    cto: i32,
    sync: bool,
}

struct Track<'a> {
    kind: Kind,
    id: u32,
    timescale: u32,
    trak: &'a [u8],
    codec: String,
    width: u32,
    height: u32,
    samples: Vec<Sample>,
}

/// Its handler: `vide`, `soun` or other.
fn handler(trak: &[u8]) -> Option<[u8; 4]> {
    path(trak, &[b"mdia", b"hdlr"])?.get(8..12)?.try_into().ok()
}

fn read_track(trak: &[u8], file_len: u64) -> Result<Option<Track<'_>>, NotRemuxed> {
    use NotRemuxed::*;
    let kind = match handler(trak).ok_or(Unreadable)? {
        h if &h == b"vide" => Kind::Video,
        h if &h == b"soun" => Kind::Audio,
        _ => return Ok(None),
    };
    let tkhd = child(trak, b"tkhd").ok_or(Unreadable)?;
    let v1 = tkhd.first() == Some(&1);
    let id = u32_at(tkhd, if v1 { 20 } else { 12 }).ok_or(Unreadable)?;
    // tkhd's matrix, then its width and height (16.16).
    let matrix_at = if v1 { 52 } else { 40 };
    let b = u32_at(tkhd, matrix_at + 4).ok_or(Unreadable)?;
    let mut width = u32_at(tkhd, matrix_at + 36).ok_or(Unreadable)? >> 16;
    let mut height = u32_at(tkhd, matrix_at + 40).ok_or(Unreadable)? >> 16;
    if b != 0 {
        // Turned a quarter: shown as tall as it is wide.
        std::mem::swap(&mut width, &mut height);
    }
    let mdhd = path(trak, &[b"mdia", b"mdhd"]).ok_or(Unreadable)?;
    let timescale = u32_at(mdhd, if mdhd.first() == Some(&1) { 20 } else { 12 }).ok_or(Unreadable)?;
    let stbl = path(trak, &[b"mdia", b"minf", b"stbl"]).ok_or(Unreadable)?;
    let stsd = child(stbl, b"stsd").ok_or(Unreadable)?;
    if u32_at(stsd, 4) != Some(1) {
        return Err(Unsupported);
    }
    let entries = boxes(stsd.get(8..).ok_or(Unreadable)?).ok_or(Unreadable)?;
    let (entry_kind, entry) = *entries.first().ok_or(Unreadable)?;
    let codec = match kind {
        Kind::Video => video_codec(&entry_kind, entry).ok_or(Unsupported)?,
        Kind::Audio => audio_codec(&entry_kind, entry).ok_or(Unsupported)?,
    };
    if timescale == 0 {
        return Err(Unreadable);
    }
    let samples = samples(stbl, file_len, kind).ok_or(Unreadable)?;
    if samples.is_empty() {
        return Err(Unsuitable);
    }
    Ok(Some(Track { kind, id, timescale, trak, codec, width, height, samples }))
}

/// The track's samples, from its sample tables.
fn samples(stbl: &[u8], file_len: u64, kind: Kind) -> Option<Vec<Sample>> {
    // Sizes.
    let stsz = child(stbl, b"stsz")?;
    let (fixed, count) = (u32_at(stsz, 4)?, u32_at(stsz, 8)?);
    if count == 0 || count > 2_000_000 {
        return Some(Vec::new());
    }
    let sizes: Vec<u32> = if fixed != 0 { vec![fixed; count as usize] } else { words(table(stsz, 8, 4)?.1) };
    let count = sizes.len();
    // Chunk offsets.
    let chunks: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
        words(table(stco, 4, 4)?.1).into_iter().map(u64::from).collect()
    } else {
        let words = words(table(child(stbl, b"co64")?, 4, 8)?.1);
        words.as_chunks::<2>().0.iter().map(|[high, low]| (u64::from(*high) << 32) | u64::from(*low)).collect()
    };
    // Samples per chunk.
    let (_, stsc) = table(child(stbl, b"stsc")?, 4, 12)?;
    let stsc = words(stsc);
    let runs: Vec<(u32, u32)> = stsc.as_chunks::<3>().0.iter().map(|[first, per, _]| (*first, *per)).collect();
    let mut offsets = Vec::with_capacity(count);
    for (i, chunk_offset) in chunks.iter().enumerate() {
        let chunk = i as u32 + 1;
        let per = runs.iter().rev().find(|(first, _)| *first <= chunk)?.1;
        let mut at = *chunk_offset;
        for _ in 0..per {
            let Some(size) = sizes.get(offsets.len()) else { break };
            offsets.push(at);
            at += u64::from(*size);
        }
    }
    if offsets.len() != count {
        return None;
    }
    // Durations.
    let (_, stts) = table(child(stbl, b"stts")?, 4, 8)?;
    let mut durations = Vec::with_capacity(count);
    for &[n, d] in words(stts).as_chunks::<2>().0 {
        let n = (n as usize).min(count - durations.len());
        durations.extend(std::iter::repeat_n(d, n));
    }
    // A table a little short: the last duration goes on.
    let last = *durations.last()?;
    durations.resize(count, last);
    // Composition offsets.
    let mut ctos = vec![0i32; count];
    if let Some(ctts) = child(stbl, b"ctts") {
        let (_, e) = table(ctts, 4, 8)?;
        let mut at = 0;
        for &[n, o] in words(e).as_chunks::<2>().0 {
            let end = at + (n as usize).min(count - at);
            ctos[at..end].fill(o as i32);
            at = end;
        }
    }
    // Key frames: all of them, without a table (and in audio).
    let syncs: Option<Vec<u32>> = match (kind, child(stbl, b"stss")) {
        (Kind::Video, Some(stss)) => Some(words(table(stss, 4, 4)?.1)),
        _ => None,
    };
    let mut out = Vec::with_capacity(count);
    let mut dts = 0u64;
    for i in 0..count {
        let (offset, size) = (offsets[i], sizes[i]);
        if offset.checked_add(u64::from(size))? > file_len {
            return None;
        }
        let sync = syncs.as_ref().is_none_or(|s| s.binary_search(&(i as u32 + 1)).is_ok());
        out.push(Sample { offset, size, dts, duration: durations[i], cto: ctos[i], sync });
        dts += u64::from(durations[i]);
    }
    Some(out)
}

/// A visual sample entry's codec: `avc1.PPCCLL` or HEVC's long form.
fn video_codec(kind: &[u8; 4], entry: &[u8]) -> Option<String> {
    // 78 bytes of VisualSampleEntry before its boxes.
    let inner = boxes(entry.get(78..)?)?;
    let find = |k: &[u8; 4]| inner.iter().find(|(kind, _)| kind == k).map(|(_, b)| *b);
    match kind {
        b"avc1" | b"avc3" => {
            let c = find(b"avcC")?;
            Some(format!("{}.{:02x}{:02x}{:02x}", std::str::from_utf8(kind).ok()?, c.get(1)?, c.get(2)?, c.get(3)?))
        }
        b"hvc1" | b"hev1" => {
            let c = find(b"hvcC")?;
            let first = *c.get(1)?;
            let space = ["", "A", "B", "C"][usize::from(first >> 6)];
            let tier = if first & 0x20 != 0 { 'H' } else { 'L' };
            let profile = first & 0x1f;
            let compat = u32_at(c, 2)?.reverse_bits();
            let constraints = c.get(6..12)?;
            let kept = constraints.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
            let mut out =
                format!("{}.{space}{profile}.{compat:X}.{tier}{}", std::str::from_utf8(kind).ok()?, c.get(12)?);
            for b in &constraints[..kept] {
                let _ = write!(out, ".{b:X}");
            }
            Some(out)
        }
        _ => None,
    }
}

/// An AAC sample entry's codec, `mp4a.40.<object type>`; nothing else.
fn audio_codec(kind: &[u8; 4], entry: &[u8]) -> Option<String> {
    if kind != b"mp4a" || u16_at(entry, 8)? != 0 {
        // Not AAC, or QuickTime's sound description (version 1 or 2), which
        // fragmented MP4 players don't take.
        return None;
    }
    let esds = child(entry.get(28..)?, b"esds")?;
    let mut d = esds.get(4..)?;
    // ES_Descriptor, DecoderConfigDescriptor, DecoderSpecificInfo.
    let descriptor = |d: &[u8]| -> Option<(u8, usize, usize)> {
        let tag = *d.first()?;
        let mut len = 0usize;
        for i in 1..5 {
            let b = *d.get(i)?;
            len = (len << 7) | usize::from(b & 0x7f);
            if b & 0x80 == 0 {
                return Some((tag, i + 1, len));
            }
        }
        None
    };
    let (tag, head, _) = descriptor(d)?;
    if tag != 3 {
        return None;
    }
    let flags = *d.get(head + 2)?;
    let mut skip = head + 3;
    if flags & 0x80 != 0 {
        skip += 2;
    }
    if flags & 0x40 != 0 {
        skip += 1 + usize::from(*d.get(skip)?);
    }
    if flags & 0x20 != 0 {
        skip += 2;
    }
    d = d.get(skip..)?;
    let (tag, head, _) = descriptor(d)?;
    if tag != 4 || *d.get(head)? != 0x40 {
        return None;
    }
    d = d.get(head + 13..)?;
    let aot = match descriptor(d) {
        Some((5, head, _)) => match d.get(head).map(|b| b >> 3) {
            Some(31) => 32 + ((u16_at(d, head)? >> 5) & 0x3f) as u8,
            Some(0) | None => 2,
            Some(aot) => aot,
        },
        _ => 2,
    };
    Some(format!("mp4a.40.{aot}"))
}

// ---------------------------------------------------------------------------
// Remuxing

/// `file`, an MP4 or MOV, as one fragmented MP4 with its index; or why not.
pub fn remux(file: &[u8]) -> Result<Remuxed, NotRemuxed> {
    use NotRemuxed::*;
    let top = boxes(file).ok_or(Unreadable)?;
    let moov = top.iter().find(|(k, _)| k == b"moov").map(|(_, b)| *b).ok_or(Unreadable)?;
    if child(moov, b"mvex").is_some() {
        return Err(Unsuitable);
    }
    let mut video = None;
    let mut audio = None;
    for (kind, trak) in boxes(moov).ok_or(Unreadable)? {
        if &kind != b"trak" {
            continue;
        }
        match read_track(trak, file.len() as u64) {
            Ok(Some(t)) if t.kind == Kind::Video && video.is_none() => video = Some(t),
            Ok(Some(t)) if t.kind == Kind::Audio && audio.is_none() => audio = Some(t),
            Ok(_) => {}
            // Audio we can't carry would leave the video silent: keep the file.
            Err(why) => return Err(why),
        }
    }
    let video = video.ok_or(Unsupported)?;
    if video.width == 0 || video.height == 0 || audio.as_ref().is_some_and(|a| a.id == video.id) {
        return Err(Unreadable);
    }
    let tracks: Vec<&Track> = std::iter::once(&video).chain(audio.as_ref()).collect();

    // Where each segment starts in the video, at key frames.
    let total = video.samples.last().map_or(0, |s| s.dts + u64::from(s.duration));
    let seconds = total as f64 / f64::from(video.timescale);
    let target = SEGMENT_SECONDS.max(seconds / (MAX_SEGMENTS as f64 - 8.0));
    let mut starts = vec![0usize];
    for (i, s) in video.samples.iter().enumerate().skip(1) {
        let since = (s.dts - video.samples[*starts.last().unwrap()].dts) as f64 / f64::from(video.timescale);
        if s.sync && since >= target {
            starts.push(i);
        }
    }
    if !video.samples[0].sync || starts.len() > MAX_SEGMENTS {
        return Err(Unsuitable);
    }
    // Each segment's video samples, and its end in seconds.
    let bounds: Vec<(usize, usize)> = starts
        .iter()
        .enumerate()
        .map(|(k, &a)| (a, starts.get(k + 1).copied().unwrap_or(video.samples.len())))
        .collect();
    let ends: Vec<f64> = bounds
        .iter()
        .map(|&(_, b)| video.samples.get(b).map_or(f64::INFINITY, |s| s.dts as f64 / f64::from(video.timescale)))
        .collect();

    let mut out = Vec::with_capacity(file.len() + 4096);
    init_segment(&mut out, moov, &tracks).ok_or(Unreadable)?;
    let init = out.len() as u64;
    let mut segments = Vec::with_capacity(bounds.len());
    let mut audio_at = 0usize;
    for (k, &(a, b)) in bounds.iter().enumerate() {
        let mut parts: Vec<(&Track, &[Sample])> = vec![(&video, &video.samples[a..b])];
        if let Some(audio) = &audio {
            let from = audio_at;
            while audio_at < audio.samples.len()
                && (audio.samples[audio_at].dts as f64 / f64::from(audio.timescale)) < ends[k]
            {
                audio_at += 1;
            }
            if audio_at > from {
                parts.push((audio, &audio.samples[from..audio_at]));
            }
        }
        let start = out.len();
        fragment(&mut out, k as u32 + 1, &parts, file);
        let ticks: u64 = video.samples[a..b].iter().map(|s| u64::from(s.duration)).sum();
        let ms = ((ticks as f64 * 1000.0 / f64::from(video.timescale)).round() as u32).max(1);
        segments.push((ms, (out.len() - start) as u64));
    }
    let codecs = tracks.iter().map(|t| t.codec.as_str()).collect::<Vec<_>>().join(",");
    let index = HlsIndex { width: video.width, height: video.height, codecs, init, segments };
    if index.encode().len() > MAX_INDEX_BYTES {
        return Err(Unsuitable);
    }
    Ok(Remuxed { bytes: out, index })
}

/// `ftyp` and a `moov` of the tracks, without samples, with `mvex`.
fn init_segment(out: &mut Vec<u8>, moov: &[u8], tracks: &[&Track]) -> Option<()> {
    put_raw(out, b"ftyp", b"iso6\0\0\0\0iso6isomiso5dashmp41");
    let mvhd = child(moov, b"mvhd")?;
    let next_id = tracks.iter().map(|t| t.id).max()? + 1;
    let mut trak_bodies = Vec::new();
    for t in tracks {
        let mut trak = Vec::new();
        for (kind, body) in boxes(t.trak)? {
            match &kind {
                b"tkhd" | b"edts" => put_raw(&mut trak, &kind, body),
                b"mdia" => put_box(&mut trak, b"mdia", |m| mdia(m, body)),
                _ => {}
            }
        }
        trak_bodies.push(trak);
    }
    put_box(out, b"moov", |m| {
        put_raw(m, b"mvhd", mvhd);
        // mvhd's next track id is its last field.
        let at = m.len() - 4;
        m[at..].copy_from_slice(&next_id.to_be_bytes());
        for trak in &trak_bodies {
            put_raw(m, b"trak", trak);
        }
        put_box(m, b"mvex", |x| {
            for t in tracks {
                put_full(x, b"trex", 0, 0, |b| {
                    for v in [t.id, 1, 0, 0, 0] {
                        b.extend_from_slice(&v.to_be_bytes());
                    }
                });
            }
        });
    });
    Some(())
}

/// A track's `mdia`, its sample tables emptied.
fn mdia(out: &mut Vec<u8>, body: &[u8]) {
    for (kind, body) in boxes(body).unwrap_or_default() {
        match &kind {
            b"minf" => put_box(out, b"minf", |m| {
                for (kind, body) in boxes(body).unwrap_or_default() {
                    if &kind == b"stbl" {
                        put_box(m, b"stbl", |s| {
                            if let Some(stsd) = child(body, b"stsd") {
                                put_raw(s, b"stsd", stsd);
                            }
                            put_full(s, b"stts", 0, 0, |b| b.extend_from_slice(&[0; 4]));
                            put_full(s, b"stsc", 0, 0, |b| b.extend_from_slice(&[0; 4]));
                            put_full(s, b"stsz", 0, 0, |b| b.extend_from_slice(&[0; 8]));
                            put_full(s, b"stco", 0, 0, |b| b.extend_from_slice(&[0; 4]));
                        });
                    } else if matches!(&kind, b"vmhd" | b"smhd" | b"dinf") {
                        put_raw(m, &kind, body);
                    }
                }
            }),
            b"mdhd" | b"hdlr" => put_raw(out, &kind, body),
            _ => {}
        }
    }
}

/// One `moof` and its `mdat`: each part's samples, as a track fragment.
fn fragment(out: &mut Vec<u8>, sequence: u32, parts: &[(&Track, &[Sample])], file: &[u8]) {
    let moof_start = out.len();
    let mut data_offsets = Vec::new();
    put_box(out, b"moof", |m| {
        put_full(m, b"mfhd", 0, 0, |b| b.extend_from_slice(&sequence.to_be_bytes()));
        for (track, samples) in parts {
            put_box(m, b"traf", |t| {
                // default-base-is-moof
                put_full(t, b"tfhd", 0, 0x02_0000, |b| b.extend_from_slice(&track.id.to_be_bytes()));
                put_full(t, b"tfdt", 1, 0, |b| b.extend_from_slice(&samples[0].dts.to_be_bytes()));
                // data offset, and each sample's duration, size, flags and
                // composition offset (signed, version 1).
                put_full(t, b"trun", 1, 0x0001 | 0x0100 | 0x0200 | 0x0400 | 0x0800, |b| {
                    b.extend_from_slice(&(samples.len() as u32).to_be_bytes());
                    // Where the data offset goes, in `out`, filled in below.
                    data_offsets.push(b.len());
                    b.extend_from_slice(&[0; 4]);
                    for s in *samples {
                        let flags: u32 = if s.sync { 0x0200_0000 } else { 0x0101_0000 };
                        for v in [s.duration, s.size, flags, s.cto as u32] {
                            b.extend_from_slice(&v.to_be_bytes());
                        }
                    }
                });
            });
        }
    });
    let moof_len = out.len() - moof_start;
    // Each part's data, in order, after the mdat's header.
    let mut at = moof_len + 8;
    for (i, (_, samples)) in parts.iter().enumerate() {
        let field = data_offsets[i];
        out[field..field + 4].copy_from_slice(&(at as u32).to_be_bytes());
        at += samples.iter().map(|s| s.size as usize).sum::<usize>();
    }
    put_box(out, b"mdat", |d| {
        for (_, samples) in parts {
            for s in *samples {
                d.extend_from_slice(&file[s.offset as usize..s.offset as usize + s.size as usize]);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    fn top(bytes: &[u8]) -> Vec<String> {
        boxes(bytes).unwrap().iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect()
    }

    /// Every sample of `track` in `file`, as (dts, bytes), in order.
    fn source_samples(file: &[u8], kind: Kind) -> Vec<(u64, Vec<u8>)> {
        let moov = child(file, b"moov").unwrap();
        let trak = boxes(moov)
            .unwrap()
            .into_iter()
            .filter(|(k, _)| k == b"trak")
            .map(|(_, b)| read_track(b, file.len() as u64).unwrap().unwrap())
            .find(|t| t.kind == kind)
            .unwrap();
        trak.samples
            .iter()
            .map(|s| (s.dts, file[s.offset as usize..s.offset as usize + s.size as usize].to_vec()))
            .collect()
    }

    /// Every sample of track `id` in a fragmented file, read back from its
    /// fragments as a player would: (dts, bytes).
    fn fragment_samples(bytes: &[u8], id: u32) -> Vec<(u64, Vec<u8>)> {
        let mut out = Vec::new();
        let mut at = 0;
        for (kind, body) in boxes(bytes).unwrap() {
            let header = 8;
            if &kind == b"moof" {
                for (k, traf) in boxes(body).unwrap() {
                    if &k != b"traf" {
                        continue;
                    }
                    let tfhd = child(traf, b"tfhd").unwrap();
                    if u32_at(tfhd, 4).unwrap() != id {
                        continue;
                    }
                    let mut dts = u64_at(child(traf, b"tfdt").unwrap(), 4).unwrap();
                    let trun = child(traf, b"trun").unwrap();
                    let count = u32_at(trun, 4).unwrap() as usize;
                    let mut data = at + u32_at(trun, 8).unwrap() as usize;
                    for i in 0..count {
                        let e = 12 + i * 16;
                        let (duration, size) = (u32_at(trun, e).unwrap(), u32_at(trun, e + 4).unwrap() as usize);
                        out.push((dts, bytes[data..data + size].to_vec()));
                        data += size;
                        dts += u64::from(duration);
                    }
                }
            }
            at += header + body.len();
        }
        out
    }

    #[test]
    fn remuxes_h264_and_aac_sample_for_sample() {
        let file = fixture("video-h264-aac.mp4");
        let remuxed = remux(&file).unwrap();
        let index = &remuxed.index;
        assert_eq!(index.total(), remuxed.bytes.len() as u64);
        assert_eq!((index.width, index.height), (64, 36));
        assert!(index.codecs.starts_with("avc1.64") && index.codecs.ends_with(",mp4a.40.2"), "{}", index.codecs);
        // 12 s, a key frame every 2 s, segments of at least 4 s.
        assert_eq!(index.segments.iter().map(|(ms, _)| ms).collect::<Vec<_>>(), [&4000, &4000, &4000]);
        let names = top(&remuxed.bytes);
        assert_eq!(names[..2], ["ftyp", "moov"]);
        assert_eq!(names[2..].chunks(2).filter(|p| p == &["moof", "mdat"]).count(), 3);
        assert_eq!(boxes(&remuxed.bytes[..index.init as usize]).unwrap().len(), 2, "the init is ftyp and moov");
        // Every sample, in order and at its time, byte for byte.
        assert_eq!(fragment_samples(&remuxed.bytes, 1), source_samples(&file, Kind::Video));
        assert_eq!(fragment_samples(&remuxed.bytes, 2), source_samples(&file, Kind::Audio));
        // Each segment starts with a moof, at a key frame.
        let mut at = index.init as usize;
        for (_, len) in &index.segments {
            assert_eq!(&remuxed.bytes[at + 4..at + 8], b"moof");
            at += *len as usize;
        }
    }

    #[test]
    fn leaves_out_metadata_and_samples_tables() {
        let file = fixture("video-h264-aac.mp4");
        assert!(file.windows(4).any(|w| w == b"udta"), "the fixture has a recorded place");
        let remuxed = remux(&file).unwrap();
        let init = &remuxed.bytes[..remuxed.index.init as usize];
        assert!(!init.windows(4).any(|w| w == b"udta" || w == b"meta"));
        assert!(!init.windows(9).any(|w| w == b"+40.0000-"));
        let moov = child(init, b"moov").unwrap();
        assert!(child(moov, b"mvex").is_some());
        assert_eq!(u32_at(child(moov, b"mvhd").unwrap(), 96), Some(3), "next track id");
    }

    #[test]
    fn remuxes_mov_without_sound_and_hevc() {
        let silent = remux(&fixture("video-silent.mov")).unwrap();
        assert!(silent.index.codecs.starts_with("avc1.") && !silent.index.codecs.contains(','));
        assert_eq!((silent.index.width, silent.index.height), (48, 64));
        assert_eq!(silent.index.segments.len(), 1);
        let hevc = remux(&fixture("video-hevc.mp4")).unwrap();
        assert!(
            hevc.index.codecs.starts_with("hvc1.1.6.L") && hevc.index.codecs.ends_with(",mp4a.40.2"),
            "{}",
            hevc.index.codecs
        );
    }

    #[test]
    fn keeps_what_it_cant_remux() {
        assert_eq!(remux(&fixture("video-fragmented.mp4")).err(), Some(NotRemuxed::Unsuitable));
        assert_eq!(remux(b"not a video").err(), Some(NotRemuxed::Unreadable));
        assert_eq!(remux(&[]).err(), Some(NotRemuxed::Unreadable));
        // Cut short: its samples are past the end.
        let file = fixture("video-h264-aac.mp4");
        assert!(remux(&file[..file.len() / 2]).is_err());
        // Any prefix at all is refused or remuxed, never a panic.
        for end in (0..file.len()).step_by(997) {
            let _ = remux(&file[..end]);
        }
    }

    fn index() -> HlsIndex {
        HlsIndex {
            width: 1080,
            height: 1920,
            codecs: "avc1.64001f,mp4a.40.2".into(),
            init: 900,
            segments: vec![(4000, 1_000_000), (2500, 400_000)],
        }
    }

    #[test]
    fn index_round_trips_and_refuses_anything_else() {
        let line = index().encode();
        assert_eq!(line, "1;1080x1920;avc1.64001f,mp4a.40.2;900;4000:1000000,2500:400000");
        assert_eq!(HlsIndex::parse(&line), Some(index()));
        assert_eq!(index().total(), 1_400_900);
        for bad in [
            "",
            "2;1080x1920;avc1;900;4000:1",
            "1;1080x1920;avc1;900;4000:1;",
            "1;0x1920;avc1;900;4000:1",
            "1;1080x1920;;900;4000:1",
            "1;1080x1920;avc1\";900;4000:1",
            "1;1080x1920;avc1\n#EXT;900;4000:1",
            "1;1080x1920;avc1;0;4000:1",
            "1;1080x1920;avc1;900;",
            "1;1080x1920;avc1;900;0:1",
            "1;1080x1920;avc1;900;4000:0",
            "1;1080x1920;avc1;0900;4000:1",
            "1;1080x1920;avc1;-900;4000:1",
            "1;1080x1920;avc1;900;4000:1,",
        ] {
            assert_eq!(HlsIndex::parse(bad), None, "{bad:?}");
        }
        let many = format!("1;1x1;avc1;1;{}", vec!["1:1"; MAX_SEGMENTS + 1].join(","));
        assert_eq!(HlsIndex::parse(&many), None);
    }

    #[test]
    fn writes_playlists() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            master_playlist(&index(), id),
            format!(
                "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n\
                 #EXT-X-STREAM-INF:BANDWIDTH=2000000,AVERAGE-BANDWIDTH=1724185,RESOLUTION=1080x1920,CODECS=\"avc1.64001f,mp4a.40.2\"\n{id}.v.m3u8\n"
            )
        );
        assert_eq!(
            media_playlist(&index(), id),
            format!(
                "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n\
                 #EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MAP:URI=\"{id}.mp4\",BYTERANGE=\"900@0\"\n\
                 #EXTINF:4.000,\n#EXT-X-BYTERANGE:1000000@900\n{id}.mp4\n\
                 #EXTINF:2.500,\n#EXT-X-BYTERANGE:400000@1000900\n{id}.mp4\n#EXT-X-ENDLIST\n"
            )
        );
        // As Allthing's own player reads them.
        let master = crate::hls::parse_master(&master_playlist(&index(), id)).unwrap();
        assert_eq!((master.variants[0].width, master.variants[0].height), (1080, 1920));
        assert_eq!(crate::hls::parse_media(&media_playlist(&index(), id)).unwrap().duration(), 6.5);
    }
}

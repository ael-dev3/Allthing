// SPDX-License-Identifier: AGPL-3.0-or-later
//! What a picture says about where and when it was taken, taken out before
//! it's uploaded. A phone's photo carries its camera's EXIF data, often with
//! the place it was taken (GPS), the time, and the phone's make and model;
//! an image edited on a computer may carry XMP or IPTC text too. The Worker
//! keeps an attachment as it's sent, and anyone can read it at its URL, so
//! the browser takes these out first ([`without_metadata`]), and keeps the
//! picture's own data byte for byte:
//!
//! - JPEG: its APP1 segments (EXIF and XMP) and APP13 (IPTC, Photoshop's),
//!   and what follows the picture's end (a phone may append more pictures
//!   there, a depth or HDR map, with EXIF of their own). A photo's
//!   orientation, an EXIF tag a phone sets rather than turning the picture,
//!   is kept in a new EXIF segment that holds nothing else, so the photo
//!   still shows the right way up.
//! - PNG: its `eXIf`, `tEXt`, `iTXt` and `zTXt` chunks.
//! - WebP: its `EXIF` and `XMP ` chunks, with the file's size and the
//!   `VP8X` chunk's flags for them set to match.
//!
//! GIFs and videos go as they are: a GIF has no EXIF, and taking a video's
//! metadata out (an iPhone's says where it was filmed) would mean rewriting
//! its container, which the browser doesn't do.

use crate::media::MediaType;

/// Whether the browser takes a file of this type apart before it's
/// uploaded ([`without_metadata`]).
pub fn stripped(media: MediaType) -> bool {
    matches!(media, MediaType::Jpeg | MediaType::Png | MediaType::Webp)
}

/// A picture put back together without its metadata, or `None` when it
/// can't be read as the type it says it is. Types that aren't
/// [`stripped`] come back as they are.
pub fn without_metadata(media: MediaType, bytes: &[u8]) -> Option<Vec<u8>> {
    match media {
        MediaType::Jpeg => jpeg(bytes),
        MediaType::Png => png(bytes),
        MediaType::Webp => webp(bytes),
        MediaType::Gif | MediaType::Mp4 | MediaType::Mov | MediaType::Webm => Some(bytes.to_vec()),
    }
}

/// EXIF's orientation tag.
const ORIENTATION: u16 = 0x0112;

fn jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut at = 2;
    let mut turned = false;
    loop {
        // A marker, after any fill bytes before it.
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        while bytes.get(at + 1) == Some(&0xFF) {
            at += 1;
        }
        let marker = *bytes.get(at + 1)?;
        match marker {
            // The picture's end: what follows isn't this picture.
            0xD9 => {
                out.extend_from_slice(&[0xFF, 0xD9]);
                return Some(out);
            }
            // Markers that stand alone, without a length.
            0x01 | 0xD0..=0xD7 => {
                out.extend_from_slice(&[0xFF, marker]);
                at += 2;
                continue;
            }
            0x00 | 0xD8 => return None,
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes([*bytes.get(at + 2)?, *bytes.get(at + 3)?]));
        if length < 2 {
            return None;
        }
        let end = at + 2 + length;
        let segment = bytes.get(at..end)?;
        match marker {
            // EXIF and XMP: only the orientation is kept, once.
            0xE1 => {
                if let Some(orientation) = exif_orientation(&segment[4..]).filter(|o| (2..=8).contains(o))
                    && !turned
                {
                    out.extend_from_slice(&orientation_only(orientation));
                    turned = true;
                }
            }
            // IPTC.
            0xED => {}
            _ => out.extend_from_slice(segment),
        }
        at = end;
        // A scan: its header, then the picture's coded data, as it is, up
        // to the next marker (a byte 0xFF that isn't followed by 0x00, a
        // stuffed 0xFF, or by a restart marker). A file cut short in a scan
        // ends there.
        if marker == 0xDA {
            let mut next = at;
            loop {
                match (bytes.get(next), bytes.get(next + 1)) {
                    (Some(0xFF), Some(0x00 | 0xD0..=0xD7)) => next += 2,
                    (Some(0xFF), Some(_)) => break,
                    (Some(_), _) => next += 1,
                    (None, _) => return Some([out, bytes[at..].to_vec()].concat()),
                }
            }
            out.extend_from_slice(&bytes[at..next]);
            at = next;
        }
    }
}

/// The orientation an EXIF segment's first directory gives, if it gives
/// one: 1, as taken, to 8.
fn exif_orientation(payload: &[u8]) -> Option<u16> {
    let tiff = payload.strip_prefix(b"Exif\0\0")?;
    let big = match tiff.get(..4)? {
        b"MM\0*" => true,
        b"II*\0" => false,
        _ => return None,
    };
    let u16_at = |at: usize| {
        let b = tiff.get(at..at.checked_add(2)?)?;
        Some(if big { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) })
    };
    let u32_at = |at: usize| {
        let b: [u8; 4] = tiff.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    };
    let directory = usize::try_from(u32_at(4)?).ok()?;
    let entries = usize::from(u16_at(directory)?);
    (0..entries).find_map(|i| {
        let entry = directory.checked_add(2 + 12 * i)?;
        // A SHORT: its value is the first two bytes of the entry's last four.
        (u16_at(entry)? == ORIENTATION && u16_at(entry + 2)? == 3).then(|| u16_at(entry + 8)).flatten()
    })
}

/// An EXIF segment that holds an orientation and nothing else: a
/// big-endian TIFF header and one directory of one entry.
fn orientation_only(orientation: u16) -> Vec<u8> {
    let mut segment = vec![0xFF, 0xE1, 0x00, 0x22];
    segment.extend_from_slice(b"Exif\0\0MM\0*\0\0\0\x08");
    segment.extend_from_slice(&1u16.to_be_bytes());
    segment.extend_from_slice(&ORIENTATION.to_be_bytes());
    segment.extend_from_slice(&3u16.to_be_bytes());
    segment.extend_from_slice(&1u32.to_be_bytes());
    segment.extend_from_slice(&orientation.to_be_bytes());
    segment.extend_from_slice(&[0, 0]);
    segment.extend_from_slice(&0u32.to_be_bytes());
    segment
}

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

fn png(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut at = PNG_SIGNATURE.len();
    if !bytes.starts_with(PNG_SIGNATURE) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(PNG_SIGNATURE);
    loop {
        let length = usize::try_from(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?)).ok()?;
        let kind = bytes.get(at + 4..at + 8)?;
        // Its length, type, data and CRC, copied whole.
        let end = at.checked_add(12)?.checked_add(length)?;
        let chunk = bytes.get(at..end)?;
        if !matches!(kind, b"eXIf" | b"tEXt" | b"iTXt" | b"zTXt") {
            out.extend_from_slice(chunk);
        }
        at = end;
        // What may follow the end isn't part of the picture.
        if kind == b"IEND" {
            return Some(out);
        }
    }
}

/// The `VP8X` flags that say a WebP has EXIF and XMP chunks.
const WEBP_EXIF: u8 = 0x08;
const WEBP_XMP: u8 = 0x04;

fn webp(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WEBP" {
        return None;
    }
    let size = usize::try_from(u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?)).ok()?;
    // The chunks within the RIFF's size; anything after it isn't the file's.
    let chunks = bytes.get(12..size.checked_add(8)?.min(bytes.len()))?;
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(b"RIFF\0\0\0\0WEBP");
    let mut at = 0;
    while at < chunks.len() {
        let kind = chunks.get(at..at + 4)?;
        let length = usize::try_from(u32::from_le_bytes(chunks.get(at + 4..at + 8)?.try_into().ok()?)).ok()?;
        let data_end = at.checked_add(8)?.checked_add(length)?;
        let chunk = chunks.get(at..data_end)?;
        // A chunk of odd length has a byte of padding after it, which a
        // file's last chunk may lack.
        let padded = length % 2 == 1;
        match kind {
            b"EXIF" | b"XMP " => {}
            _ => {
                let start = out.len();
                out.extend_from_slice(chunk);
                if kind == b"VP8X" && length > 0 {
                    out[start + 8] &= !(WEBP_EXIF | WEBP_XMP);
                }
                if padded {
                    out.push(0);
                }
            }
        }
        at = data_end + usize::from(padded);
    }
    let riff = u32::try_from(out.len() - 8).ok()?;
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JPEG segment: its marker, length and payload.
    fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0xFF, marker];
        out.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// EXIF as a phone writes it, little-endian or big: the camera's make
    /// and model, an orientation, and a GPS directory with where it was
    /// taken (52°22'N, 4°53'E).
    fn exif(big: bool, orientation: u16) -> Vec<u8> {
        let u16b = |v: u16| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let u32b = |v: u32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let mut tiff = Vec::new();
        tiff.extend_from_slice(if big { b"MM\0*" } else { b"II*\0" });
        tiff.extend_from_slice(&u32b(8));
        // IFD0 at 8: Make, Model, Orientation and the GPS pointer.
        let entries: u16 = 4;
        let ifd0_end = 8 + 2 + 12 * u32::from(entries) + 4;
        let (make_at, model_at, gps_at) = (ifd0_end, ifd0_end + 8, ifd0_end + 24);
        tiff.extend_from_slice(&u16b(entries));
        for (tag, kind, count, value) in [
            (0x010F, 2, 6, u32b(make_at)),
            (0x0110, 2, 14, u32b(model_at)),
            (ORIENTATION, 3, 1, {
                let mut v = [0; 4];
                v[..2].copy_from_slice(&u16b(orientation));
                v
            }),
            (0x8825, 4, 1, u32b(gps_at)),
        ] {
            tiff.extend_from_slice(&u16b(tag));
            tiff.extend_from_slice(&u16b(kind));
            tiff.extend_from_slice(&u32b(count));
            tiff.extend_from_slice(&value);
        }
        tiff.extend_from_slice(&u32b(0));
        tiff.extend_from_slice(b"Pixel\0\0\0");
        tiff.extend_from_slice(b"Pixel 9 Pro XL\0\0");
        // The GPS directory: N 52, E 4 (rationals elsewhere: their bytes).
        tiff.extend_from_slice(&u16b(2));
        tiff.extend_from_slice(&u16b(0x0001));
        tiff.extend_from_slice(&u16b(2));
        tiff.extend_from_slice(&u32b(2));
        tiff.extend_from_slice(b"N\0\0\0");
        tiff.extend_from_slice(&u16b(0x0003));
        tiff.extend_from_slice(&u16b(2));
        tiff.extend_from_slice(&u32b(2));
        tiff.extend_from_slice(b"E\0\0\0");
        tiff.extend_from_slice(&u32b(0));
        tiff.extend_from_slice(b"GPS 52.3667 4.8945");
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);
        payload
    }

    /// A JPEG as a phone saves it, with what it says about itself around
    /// the picture, and the picture: tables, the frame, and a scan.
    fn photo(orientation: u16) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let jfif = segment(0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0");
        let icc = segment(0xE2, b"ICC_PROFILE\0\x01\x01profile bytes");
        let tables =
            [segment(0xDB, &[0; 65]), segment(0xC0, &[8, 0, 2, 0, 3, 1, 1, 0x11, 0]), segment(0xC4, &[0; 20])].concat();
        // Two scans, as a progressive JPEG has, with a table between them;
        // the coded data has stuffed bytes and a restart marker.
        let scan = [
            segment(0xDA, &[1, 1, 0, 0, 0x3F, 0]),
            vec![0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56],
            segment(0xC4, &[0x10, 0xFF, 0xD9, 0x01]),
            segment(0xDA, &[1, 1, 0, 1, 0x3F, 0]),
            vec![0x78, 0xFF, 0x00, 0x9A],
            vec![0xFF, 0xD9],
        ]
        .concat();
        let bytes = [
            vec![0xFF, 0xD8],
            jfif.clone(),
            segment(0xE1, &exif(false, orientation)),
            segment(0xE1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>Amsterdam</x:xmpmeta>"),
            segment(0xED, b"Photoshop 3.0\08BIM\x04\x04 by-line: A. Person"),
            icc.clone(),
            // Fill bytes before a marker.
            vec![0xFF, 0xFF],
            tables.clone(),
            scan.clone(),
            // A second picture after the first's end, as a phone appends a
            // depth or HDR map, with EXIF of its own.
            vec![0xFF, 0xD8],
            segment(0xE1, &exif(false, 1)),
            vec![0xFF, 0xD9],
        ]
        .concat();
        let kept = [vec![0xFF, 0xD8], jfif, icc, tables].concat();
        (bytes, kept, scan)
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn a_jpeg_loses_its_exif_gps_xmp_and_iptc_and_keeps_its_picture() {
        let (bytes, kept, scan) = photo(1);
        for needle in [&b"Exif"[..], b"GPS", b"Pixel", b"Amsterdam", b"by-line", b"Photoshop"] {
            assert!(contains(&bytes, needle), "the fixture has {needle:?}");
        }
        let out = without_metadata(MediaType::Jpeg, &bytes).unwrap();
        // As taken (orientation 1): no EXIF at all.
        assert_eq!(out, [kept, scan.clone()].concat());
        for needle in [&b"Exif"[..], b"GPS", b"Pixel", b"Amsterdam", b"by-line", b"Photoshop", b"xap"] {
            assert!(!contains(&out, needle), "{needle:?} is gone");
        }
        // The picture's own data, byte for byte, to its end, and nothing
        // after it.
        assert!(out.ends_with(&scan));
        assert!(contains(&out, b"ICC_PROFILE"), "colour is kept");
        // Cut short in a scan's coded data (past its 10-byte header), it
        // ends there, as it was.
        let at = bytes.windows(scan.len()).position(|w| w == scan.as_slice()).unwrap();
        let cut = without_metadata(MediaType::Jpeg, &bytes[..at + 14]).unwrap();
        assert!(cut.ends_with(&scan[..14]));
        assert_eq!(cut.len(), out.len() - scan.len() + 14);
    }

    #[test]
    fn a_turned_photo_keeps_its_orientation_and_nothing_else() {
        for big in [false, true] {
            let mut bytes = photo(6).0;
            if big {
                let little = segment(0xE1, &exif(false, 6));
                let at = bytes.windows(little.len()).position(|w| w == little.as_slice()).unwrap();
                bytes.splice(at..at + little.len(), segment(0xE1, &exif(true, 6)));
            }
            assert_eq!(exif_orientation(&exif(big, 6)), Some(6));
            let out = without_metadata(MediaType::Jpeg, &bytes).unwrap();
            let kept = orientation_only(6);
            assert_eq!(kept.len(), 36);
            assert!(contains(&out, &kept));
            // It reads back as the orientation, and says nothing more.
            assert_eq!(exif_orientation(&kept[4..]), Some(6));
            for needle in [&b"GPS"[..], b"Pixel", b"Amsterdam", b"by-line"] {
                assert!(!contains(&out, needle), "{needle:?} is gone");
            }
            assert_eq!(out.windows(4).filter(|w| *w == b"Exif").count(), 1);
            // In the original's place, after JFIF.
            assert_eq!(&out[2..4], &[0xFF, 0xE0]);
        }
    }

    #[test]
    fn a_jpeg_that_cant_be_read_isnt_uploaded() {
        let (bytes, ..) = photo(1);
        assert_eq!(without_metadata(MediaType::Jpeg, &bytes[..40]), None, "cut short");
        assert_eq!(without_metadata(MediaType::Jpeg, b"not a jpeg"), None);
        assert_eq!(without_metadata(MediaType::Jpeg, &[0xFF, 0xD8, 0x00, 0x00]), None);
        // A segment whose length runs past the end.
        assert_eq!(without_metadata(MediaType::Jpeg, &[0xFF, 0xD8, 0xFF, 0xE0, 0x10, 0x00, 0x01]), None);
        assert_eq!(without_metadata(MediaType::Jpeg, &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x01]), None, "a length under 2");
        // An odd EXIF segment is dropped, and the rest kept.
        let odd = [vec![0xFF, 0xD8], segment(0xE1, b"Exif\0\0XX"), vec![0xFF, 0xD9]].concat();
        assert_eq!(without_metadata(MediaType::Jpeg, &odd), Some(vec![0xFF, 0xD8, 0xFF, 0xD9]));
        assert_eq!(exif_orientation(b"Exif\0\0MM\0*\xFF\xFF\xFF\xFF"), None, "a directory past the end");
    }

    /// A PNG chunk: its length, type, data and a CRC (any, as it's copied).
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0xC0, 0xFF, 0xEE, 0x01]);
        out
    }

    #[test]
    fn a_png_loses_its_exif_and_text_chunks() {
        let header = chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
        let data = chunk(b"IDAT", b"compressed pixels");
        let end = chunk(b"IEND", b"");
        let bytes = [
            PNG_SIGNATURE.to_vec(),
            header.clone(),
            chunk(b"tEXt", b"Author\0A. Person"),
            chunk(b"eXIf", &exif(true, 1)[6..]),
            chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0<x:xmpmeta>Amsterdam</x:xmpmeta>"),
            chunk(b"zTXt", b"Comment\0\0x"),
            chunk(b"sRGB", &[0]),
            data.clone(),
            end.clone(),
            b"trailing".to_vec(),
        ]
        .concat();
        let out = without_metadata(MediaType::Png, &bytes).unwrap();
        assert_eq!(out, [PNG_SIGNATURE.to_vec(), header, chunk(b"sRGB", &[0]), data, end].concat());
        for needle in [&b"A. Person"[..], b"GPS", b"Pixel", b"Amsterdam", b"trailing"] {
            assert!(!contains(&out, needle), "{needle:?} is gone");
        }
        assert_eq!(without_metadata(MediaType::Png, &bytes[..30]), None, "cut short");
        assert_eq!(without_metadata(MediaType::Png, b"GIF89a"), None);
    }

    /// A WebP chunk: its FourCC, length and data, padded to an even length.
    fn riff_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = kind.to_vec();
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body = chunks.concat();
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&u32::try_from(body.len() + 4).unwrap().to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn a_webp_loses_its_exif_and_xmp_and_says_so() {
        // ICC, alpha, EXIF and XMP flags set; a canvas of 1 x 1.
        let flags = 0x20 | 0x10 | WEBP_EXIF | WEBP_XMP;
        let vp8x = |flags: u8| riff_chunk(b"VP8X", &[flags, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let icc = riff_chunk(b"ICCP", b"profile");
        let image = riff_chunk(b"VP8L", b"lossless pixels");
        let bytes = riff(&[
            vp8x(flags),
            icc.clone(),
            image.clone(),
            riff_chunk(b"EXIF", &exif(false, 1)[6..]),
            riff_chunk(b"XMP ", b"<x:xmpmeta>Amsterdam</x:xmpmeta>"),
        ]);
        let out = without_metadata(MediaType::Webp, &bytes).unwrap();
        assert_eq!(out, riff(&[vp8x(0x20 | 0x10), icc, image]));
        assert_eq!(u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize, out.len() - 8);
        for needle in [&b"GPS"[..], b"Pixel", b"Amsterdam", b"XMP ", b"EXIF"] {
            assert!(!contains(&out, needle), "{needle:?} is gone");
        }
        // A simple WebP has nothing to take out.
        let simple = riff(&[riff_chunk(b"VP8 ", b"lossy pixels!")]);
        assert_eq!(without_metadata(MediaType::Webp, &simple), Some(simple.clone()));
        // The last chunk's padding may be missing.
        let unpadded = simple[..simple.len() - 1].to_vec();
        let mut fixed = unpadded.clone();
        fixed[4..8].copy_from_slice(&u32::try_from(unpadded.len() - 8).unwrap().to_le_bytes());
        assert_eq!(without_metadata(MediaType::Webp, &fixed).map(|o| o.len()), Some(simple.len()));
        assert_eq!(without_metadata(MediaType::Webp, b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(without_metadata(MediaType::Webp, &bytes[..20]), None, "cut short");
    }

    #[test]
    fn gifs_and_videos_go_as_they_are() {
        for media in [MediaType::Gif, MediaType::Mp4, MediaType::Mov, MediaType::Webm] {
            assert!(!stripped(media));
            assert_eq!(without_metadata(media, b"as it is"), Some(b"as it is".to_vec()));
        }
        assert!(stripped(MediaType::Jpeg) && stripped(MediaType::Png) && stripped(MediaType::Webp));
    }
}

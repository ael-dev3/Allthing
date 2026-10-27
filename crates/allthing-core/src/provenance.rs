// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where a cast came from: the app whose signer key signed it, and the backend
//! Allthing read it through.
//!
//! Every cast is signed by an Ed25519 signer key. Apps get signer keys added to
//! a user's account through the onchain KeyRegistry, and the add event's
//! metadata (a `SignedKeyRequestMetadata` struct, ABI-encoded) names the app's
//! own fid as `requestFid`. So "posted from app X" means "signed by a key that
//! app X requested". A cast's signer is protocol data, but which node a message
//! first reached is not recorded anywhere; see docs/PROVENANCE.md.

use serde::{Deserialize, Serialize};

/// The backend a payload was read from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Hypersnap,
    Snapchain,
    Neynar,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Hypersnap => "Hypersnap",
            Backend::Snapchain => "Snapchain",
            Backend::Neynar => "Neynar",
        }
    }
}

/// The signer that signed a cast and the app that requested it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    /// 0x-prefixed hex Ed25519 public key.
    pub signer: String,
    /// The fid of the app that requested the signer, when the key's
    /// registration carried signed-key-request metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_fid: Option<u64>,
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Decode standard or URL-safe base64, with or without padding.
pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in input.bytes() {
        let value = match byte {
            b'=' => break,
            b'-' => 62,
            b'_' => 63,
            b if b.is_ascii_whitespace() => continue,
            b => BASE64.iter().position(|&c| c == b)? as u32,
        };
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Encode as standard base64, padded.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { BASE64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Decode hex with or without a 0x prefix.
pub fn hex_decode(input: &str) -> Option<Vec<u8>> {
    let hex = input.strip_prefix("0x").or_else(|| input.strip_prefix("0X")).unwrap_or(input);
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Normalize a key or hash that an API returned as base64 or hex into 0x hex.
pub fn normalize_key(value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with("0x") || value.starts_with("0X") {
        return hex_decode(value).map(|bytes| hex_encode(&bytes));
    }
    // 32-byte keys are 64 hex characters or 44 base64 characters.
    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return hex_decode(value).map(|bytes| hex_encode(&bytes));
    }
    base64_decode(value).filter(|bytes| !bytes.is_empty()).map(|bytes| hex_encode(&bytes))
}

fn word_as_u64(word: &[u8]) -> Option<u64> {
    let (high, low) = word.split_at(24);
    high.iter().all(|b| *b == 0).then(|| low.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b)))
}

/// The `requestFid` from ABI-encoded `SignedKeyRequestMetadata(uint256
/// requestFid, address requestSigner, bytes signature, uint256 deadline)`.
///
/// `abi.encode(struct)` starts with a 32-byte offset to the tuple because the
/// struct holds dynamic `bytes`; plain `abi.encode(fields…)` does not.
pub fn request_fid_from_metadata(metadata: &[u8]) -> Option<u64> {
    if metadata.len() < 128 {
        return None;
    }
    let first = word_as_u64(&metadata[0..32])?;
    let fid = if first == 32 { word_as_u64(&metadata[32..64])? } else { first };
    (fid > 0).then_some(fid)
}

/// Read a positive requesting FID only from complete, canonically encoded
/// signed-key-request metadata. This checks the ABI shape and base64 encoding;
/// it does not independently validate the custody signature. Unknown, malformed
/// and truncated metadata remain unknown instead of becoming a no-app claim.
pub fn verified_request_fid_from_metadata(encoded: &str) -> Option<u64> {
    fn tuple_fid(fields: &[u8]) -> Option<u64> {
        let fid = word_as_u64(fields.get(..32)?)?;
        if fid == 0 || fields.get(32..44)?.iter().any(|b| *b != 0) {
            return None;
        }
        let signature_offset = usize::try_from(word_as_u64(fields.get(64..96)?)?).ok()?;
        if signature_offset != 128 {
            return None;
        }
        word_as_u64(fields.get(96..128)?)?;
        let signature_len = usize::try_from(word_as_u64(fields.get(128..160)?)?).ok()?;
        if signature_len == 0 {
            return None;
        }
        let signature_end = 160usize.checked_add(signature_len)?;
        let padded = signature_len.checked_add(31)? / 32 * 32;
        let end = 160usize.checked_add(padded)?;
        if fields.len() != end || fields.get(signature_end..end)?.iter().any(|b| *b != 0) {
            return None;
        }
        Some(fid)
    }

    let bytes = base64_decode(encoded)?;
    if base64_encode(&bytes) != encoded {
        return None;
    }
    // A flat tuple whose request FID is 32 must remain distinguishable from
    // abi.encode(struct)'s outer offset: accept only the complete valid layout.
    if word_as_u64(bytes.get(..32)?)? == 32
        && let Some(fid) = tuple_fid(bytes.get(32..)?)
    {
        return Some(fid);
    }
    tuple_fid(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(value: u64) -> Vec<u8> {
        let mut out = vec![0u8; 24];
        out.extend_from_slice(&value.to_be_bytes());
        out
    }

    fn metadata(fid: u64, with_offset: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if with_offset {
            out.extend(word(32));
        }
        out.extend(word(fid)); // requestFid
        out.extend(vec![0u8; 12]);
        out.extend([0xab; 20]); // requestSigner
        out.extend(word(128)); // offset of signature
        out.extend(word(1_700_000_000)); // deadline
        out.extend(word(65)); // signature length
        out.extend(vec![7u8; 96]); // signature, padded
        out
    }

    #[test]
    fn decodes_request_fid_in_both_encodings() {
        assert_eq!(request_fid_from_metadata(&metadata(9152, true)), Some(9152));
        assert_eq!(request_fid_from_metadata(&metadata(309_857, false)), Some(309_857));
        assert_eq!(request_fid_from_metadata(&[0u8; 10]), None);
        let mut huge = metadata(1, true);
        huge[32] = 1; // a uint256 that does not fit in u64
        assert_eq!(request_fid_from_metadata(&huge), None);
    }

    #[test]
    fn verified_metadata_requires_complete_canonical_abi_in_both_layouts() {
        let canonical = |fid, with_offset| {
            let mut bytes = metadata(fid, with_offset);
            let padding = if with_offset { 257 } else { 225 };
            bytes[padding..].fill(0);
            bytes
        };
        for (fid, with_offset) in [(9152, true), (309_857, false), (32, false)] {
            assert_eq!(verified_request_fid_from_metadata(&base64_encode(&canonical(fid, with_offset))), Some(fid));
        }
        assert_eq!(verified_request_fid_from_metadata(&base64_encode(&canonical(0, true))), None);
        let mut truncated = canonical(9152, true);
        truncated.truncate(160);
        assert_eq!(verified_request_fid_from_metadata(&base64_encode(&truncated)), None);
        let mut padded = canonical(9152, true);
        assert_eq!(verified_request_fid_from_metadata(&base64_encode(&padded)), Some(9152));
        padded[287] = 1;
        assert_eq!(verified_request_fid_from_metadata(&base64_encode(&padded)), None);
        assert_eq!(verified_request_fid_from_metadata(&(base64_encode(&canonical(9152, true)) + "ignored")), None);
    }

    #[test]
    fn decodes_a_real_signer_registration() {
        // KeyRegistry add event for one of @0xael.eth's signers, as Hypersnap's
        // /v1/onChainSignersByFid serves it: requested by fid 9152 (Farcaster).
        let metadata = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAjwAAAAAAAAAAAAAAAAALveQ3XmTo1/YR8BT7drpQNBVWWAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAZkXc9QAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABBkm8PZXmsC5a/2h4CHNOJ+1vP3cEQcvCkzn7pGxnuiV8fz0SfwJzkTYNZO3syUdrWKe2rOJmaBvrTewq+CW2LdRwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let bytes = base64_decode(metadata).unwrap();
        assert_eq!(bytes.len(), 288);
        assert_eq!(request_fid_from_metadata(&bytes), Some(9152));
    }

    #[test]
    fn base64_and_hex() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64_decode("+/8=").unwrap(), vec![0xfb, 0xff]);
        assert_eq!(base64_decode("-_8").unwrap(), vec![0xfb, 0xff]);
        assert_eq!(base64_decode("a*b"), None);
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(&[0xfb, 0xff]), "+/8=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
        assert_eq!(base64_encode(b""), "");
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(base64_decode(&base64_encode(&bytes)).unwrap(), bytes);
        assert_eq!(hex_decode("0x0aff").unwrap(), vec![0x0a, 0xff]);
        assert_eq!(hex_decode("abc"), None);
        assert_eq!(hex_encode(&[0x0a, 0xff]), "0x0aff");
        let key = [0x42u8; 32];
        let hex = hex_encode(&key);
        assert_eq!(normalize_key(&hex).as_deref(), Some(hex.as_str()));
        assert_eq!(normalize_key(&hex[2..]).as_deref(), Some(hex.as_str()));
        assert_eq!(normalize_key("QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=").as_deref(), Some(hex.as_str()));
    }
}

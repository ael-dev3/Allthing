// SPDX-License-Identifier: AGPL-3.0-or-later
//! Validating identifiers taken from URLs, shared by the browser router and
//! the Worker so both reject the same malformed input.

use crate::channels::{canonical_parent_url, is_channel_id};

/// Decode %XX escapes; `None` for malformed escapes or invalid UTF-8.
pub fn percent_decode(value: &str) -> Option<String> {
    if !value.contains('%') {
        return Some(value.to_owned());
    }
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Percent-encode everything except unreserved characters, like
/// `encodeURIComponent`.
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'!' | b'*' | b'\'' | b'(' | b')') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Cast hashes are compared and routed as 0x-prefixed lowercase hex.
pub fn normalize_hash(hash: &str) -> String {
    let lower = hash.trim().to_ascii_lowercase();
    if lower.starts_with("0x") { lower } else { format!("0x{lower}") }
}

/// A cast hash from a URL, normalized, or `None` if malformed.
pub fn hash_from_param(param: &str) -> Option<String> {
    let value = param.trim();
    let hex = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")).unwrap_or(value);
    (hex.len() == 40 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| normalize_hash(value))
}

/// A username ("dwr", "@DWR", "balajis.eth", "fid:3") from a URL, lowercased,
/// or `None` if malformed. ENS-style names and non-ASCII labels are allowed.
pub fn username_from_param(param: &str) -> Option<String> {
    let decoded = percent_decode(param)?;
    let value = decoded.trim();
    let value = value.strip_prefix('@').unwrap_or(value).to_lowercase();
    if let Some(fid) = value.strip_prefix("fid:") {
        return (!fid.is_empty() && fid.bytes().all(|b| b.is_ascii_digit())).then_some(value);
    }
    let forbidden = |c: char| c.is_whitespace() || matches!(c, '/' | '?' | '#' | '@');
    let mut chars = value.chars();
    let first_ok = chars.next().is_some_and(|c| !forbidden(c) && c != '.');
    let ok = first_ok && value.chars().count() <= 253 && !value.chars().any(forbidden);
    ok.then_some(value)
}

/// The fid in a "fid:<n>" username.
pub fn fid_username(username: &str) -> Option<u64> {
    username.strip_prefix("fid:")?.parse().ok()
}

/// A channel key from a URL: a lowercase channel id, or a canonical parent
/// URL for token and web feeds.
pub fn channel_key_from_param(param: &str) -> Option<String> {
    let decoded = percent_decode(param)?;
    let value = decoded.trim();
    if value.is_empty() {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    if is_channel_id(&lower) {
        return Some(lower);
    }
    // Anything else must look like a parent URL (scheme:rest).
    let (scheme, _) = value.split_once(':')?;
    let valid_scheme = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    valid_scheme.then(|| canonical_parent_url(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes() {
        let hash = "0x09eb33c944884cf1bbe2713ddcc87056fdd627b2";
        assert_eq!(hash_from_param(hash).as_deref(), Some(hash));
        assert_eq!(hash_from_param(&hash[2..].to_uppercase()).as_deref(), Some(hash));
        assert_eq!(hash_from_param("0x123"), None);
        assert_eq!(hash_from_param("zz".repeat(20).as_str()), None);
    }

    #[test]
    fn usernames() {
        assert_eq!(username_from_param("dwr").as_deref(), Some("dwr"));
        assert_eq!(username_from_param("@DWR").as_deref(), Some("dwr"));
        assert_eq!(username_from_param("balajis.eth").as_deref(), Some("balajis.eth"));
        assert_eq!(username_from_param("jesse.base.eth").as_deref(), Some("jesse.base.eth"));
        assert_eq!(username_from_param("%F0%9F%8C%B4.eth").as_deref(), Some("🌴.eth"));
        assert_eq!(username_from_param("fid:42").as_deref(), Some("fid:42"));
        assert_eq!(username_from_param("fid:x"), None);
        assert_eq!(username_from_param(".eth"), None);
        assert_eq!(username_from_param("a b"), None);
        assert_eq!(username_from_param("a/b"), None);
        assert_eq!(username_from_param(""), None);
        assert_eq!(username_from_param("%E0%A4%A"), None);
        assert_eq!(fid_username("fid:42"), Some(42));
        assert_eq!(fid_username("dwr"), None);
    }

    #[test]
    fn channel_keys_round_trip() {
        let token = "eip155:8453/erc20:0xabc";
        assert_eq!(percent_encode(token), "eip155%3A8453%2Ferc20%3A0xabc");
        assert_eq!(channel_key_from_param(&percent_encode(token)).as_deref(), Some(token));
        assert_eq!(channel_key_from_param(&percent_encode(&format!("chain://{token}"))).as_deref(), Some(token));
        assert_eq!(channel_key_from_param("GifArt").as_deref(), Some("gifart"));
        assert_eq!(channel_key_from_param("not a channel"), None);
        assert_eq!(channel_key_from_param("%E0%A4%A"), None);
        assert_eq!(channel_key_from_param("https://example.com/x").as_deref(), Some("https://example.com/x"));
    }

    #[test]
    fn decodes_and_encodes() {
        assert_eq!(percent_decode("a%20b").as_deref(), Some("a b"));
        assert_eq!(percent_decode("100%").as_deref(), None);
        assert_eq!(percent_encode("a b/é"), "a%20b%2F%C3%A9");
    }
}

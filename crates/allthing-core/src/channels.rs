// SPDX-License-Identifier: AGPL-3.0-or-later
//! Channel ids, parent URLs and their display labels.

use crate::model::ChannelRef;

/// Channels created before Farcaster standardised on
/// `https://warpcast.com/~/channel/<id>` parent URLs. Their casts carry these
/// legacy URLs, so a lookup assuming the standard URL finds nothing. Each entry
/// was checked against live feeds.
pub const LEGACY_CHANNEL_URLS: &[(&str, &str)] = &[
    ("farcaster", "chain://eip155:7777777/erc721:0x4f86113fc3e9783cf3ec9a552cbb566716a57628"),
    ("base", "https://onchainsummer.xyz"),
    ("ethereum", "https://ethereum.org"),
    ("memes", "chain://eip155:1/erc721:0xfd8427165df67df6d7fd689ae67c8ebf56d9ca61"),
    ("nouns", "chain://eip155:1/erc721:0x9c8ff314c9bc7f6e59a9d9225fb22946427edc03"),
    ("purple", "chain://eip155:1/erc721:0xa45662638e9f3bbb7a6fecb4b17853b7ba0f3a60"),
    ("degen", "chain://eip155:7777777/erc721:0x5d6a07d07354f8793d1ca06280c4adf04767ad7e"),
    ("dev", "chain://eip155:1/erc721:0x7dd4e31f1530ac682c8ea4d8016e95773e08d8b0"),
    ("founders", "https://farcaster.group/founders"),
    ("food", "chain://eip155:1/erc721:0xec0ba367a6edf483a252c3b093f012b9b1da8b3f"),
];

/// Channels to suggest when trending data is unavailable.
pub const DEFAULT_CHANNELS: &[&str] = &[
    "farcaster",
    "base",
    "ethereum",
    "degen",
    "memes",
    "dev",
    "founders",
    "food",
    "nouns",
    "itookaphoto",
    "art",
    "music",
    "books",
    "japan",
];

pub fn legacy_url(id: &str) -> Option<&'static str> {
    LEGACY_CHANNEL_URLS.iter().find(|(key, _)| *key == id).map(|(_, url)| *url)
}

/// A channel id: lowercase letters, digits and dashes, at most 64 characters.
pub fn is_channel_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

pub fn channel_url_for_id(id: &str) -> String {
    legacy_url(id).map_or_else(|| format!("https://warpcast.com/~/channel/{id}"), str::to_owned)
}

const CHAIN_SCHEME: &str = "chain://";

/// A CAIP-2 chain id ("eip155:8453", "solana:101") followed by an asset path.
fn is_caip_url(url: &str) -> bool {
    let Some((namespace, rest)) = url.split_once(':') else {
        return false;
    };
    let Some((reference, _)) = rest.split_once('/') else {
        return false;
    };
    (3..=8).contains(&namespace.len())
        && namespace.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && (1..=32).contains(&reference.len())
        && reference.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Token and NFT feeds are written both as "chain://eip155:8453/erc20:0x…" and
/// as bare "eip155:8453/erc20:0x…". Route keys use the bare form.
pub fn canonical_parent_url(url: &str) -> String {
    let bare = url.strip_prefix(CHAIN_SCHEME).unwrap_or(url);
    if is_caip_url(bare) { bare.to_owned() } else { url.to_owned() }
}

/// Every spelling of a parent URL that casts may have been posted under.
pub fn parent_url_variants(url: &str) -> Vec<String> {
    let bare = canonical_parent_url(url);
    if is_caip_url(&bare) { vec![format!("{CHAIN_SCHEME}{bare}"), bare] } else { vec![url.to_owned()] }
}

/// The id in a standard `https://warpcast.com/~/channel/<id>` URL.
fn standard_channel_id(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"))?;
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let rest = rest.strip_prefix("warpcast.com").or_else(|| rest.strip_prefix("farcaster.xyz"))?;
    let id = rest.strip_prefix("/~/channel/")?;
    let id = id.strip_suffix('/').unwrap_or(id);
    (!id.is_empty() && is_channel_id(id)).then(|| id.to_owned())
}

pub fn channel_id_for_url(url: &str) -> Option<String> {
    if let Some(id) = standard_channel_id(url) {
        return Some(id);
    }
    parent_url_variants(url).iter().find_map(|variant| {
        LEGACY_CHANNEL_URLS
            .iter()
            .find(|(_, legacy)| legacy.eq_ignore_ascii_case(variant))
            .map(|(id, _)| (*id).to_owned())
    })
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() > max {
        let head: String = value.chars().take(max - 1).collect();
        format!("{head}…")
    } else {
        value.to_owned()
    }
}

fn is_hex_address(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 42 && bytes[..2].eq_ignore_ascii_case(b"0x") && bytes[2..].iter().all(u8::is_ascii_hexdigit)
}

fn is_base58_address(value: &str) -> bool {
    (32..=44).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() && !matches!(b, b'0' | b'O' | b'I' | b'l'))
}

/// Short label for a parent URL that isn't a named channel (token feeds, web pages).
pub fn short_parent_url_label(url: &str) -> String {
    // chain://eip155:1/erc721:0xabc…, eip155:8453/erc20:0xabc…, solana:101/address:Abc…
    let last = url.rsplit('/').next().unwrap_or(url);
    if let Some((kind, address)) = last.rsplit_once(':') {
        let kind_ok = !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_alphanumeric());
        if kind_ok && (is_hex_address(address) || is_base58_address(address)) {
            let chars: Vec<char> = address.chars().collect();
            let head: String = chars[..6].iter().collect();
            let tail: String = chars[chars.len() - 4..].iter().collect();
            return format!("{kind}:{head}…{tail}");
        }
    }
    for scheme in ["https://", "http://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            let rest = rest.split(['?', '#']).next().unwrap_or(rest);
            return truncate(rest.trim_end_matches('/'), 32);
        }
    }
    truncate(url, 32)
}

pub fn channel_ref_from_url(url: Option<&str>) -> Option<ChannelRef> {
    let url = url.filter(|u| !u.is_empty())?;
    Some(match channel_id_for_url(url) {
        Some(id) => ChannelRef { label: id.clone(), id },
        None => ChannelRef { id: canonical_parent_url(url), label: short_parent_url_label(url) },
    })
}

/// Display label for a channel key: the id itself, or a shortened parent URL.
pub fn channel_label(key: &str) -> String {
    if is_channel_id(key) {
        key.to_owned()
    } else {
        channel_id_for_url(key).unwrap_or_else(|| short_parent_url_label(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FARCASTER_URL: &str = "chain://eip155:7777777/erc721:0x4f86113fc3e9783cf3ec9a552cbb566716a57628";
    const TOKEN: &str = "eip155:8453/erc20:0x4ed4e862860bed51a9570b96d89af5e1b0efefed";

    #[test]
    fn maps_standard_and_legacy_urls() {
        assert_eq!(channel_id_for_url("https://warpcast.com/~/channel/gifart").as_deref(), Some("gifart"));
        assert_eq!(channel_id_for_url("https://farcaster.xyz/~/channel/dev-tools/").as_deref(), Some("dev-tools"));
        assert_eq!(channel_id_for_url(FARCASTER_URL).as_deref(), Some("farcaster"));
        assert_eq!(channel_id_for_url(&FARCASTER_URL.replace("chain://", "")).as_deref(), Some("farcaster"));
        assert_eq!(channel_url_for_id("farcaster"), FARCASTER_URL);
        assert_eq!(channel_url_for_id("gifart"), "https://warpcast.com/~/channel/gifart");
        assert_eq!(channel_id_for_url("https://example.com"), None);
    }

    #[test]
    fn labels_parent_urls() {
        assert_eq!(short_parent_url_label(TOKEN), "erc20:0x4ed4…efed");
        assert_eq!(short_parent_url_label("https://example.com/some/article/"), "example.com/some/article");
        assert_eq!(
            channel_ref_from_url(Some(TOKEN)),
            Some(ChannelRef { id: TOKEN.into(), label: "erc20:0x4ed4…efed".into() })
        );
        assert_eq!(channel_ref_from_url(None), None);
        assert_eq!(channel_ref_from_url(Some(&format!("chain://{TOKEN}"))), channel_ref_from_url(Some(TOKEN)));
        assert_eq!(channel_label("dev"), "dev");
        assert_eq!(channel_label(FARCASTER_URL), "farcaster");
    }

    #[test]
    fn treats_chain_and_bare_caip_urls_as_one_feed() {
        assert_eq!(canonical_parent_url(&format!("chain://{TOKEN}")), TOKEN);
        assert_eq!(canonical_parent_url(TOKEN), TOKEN);
        assert_eq!(canonical_parent_url("https://example.com/a"), "https://example.com/a");
        assert_eq!(parent_url_variants(TOKEN), vec![format!("chain://{TOKEN}"), TOKEN.to_owned()]);
        assert_eq!(parent_url_variants("https://example.com/a"), vec!["https://example.com/a".to_owned()]);
    }

    #[test]
    fn validates_channel_ids() {
        assert!(is_channel_id("farcaster"));
        assert!(is_channel_id("dev-tools"));
        assert!(!is_channel_id("Dev"));
        assert!(!is_channel_id("-dev"));
        assert!(!is_channel_id(""));
        assert!(!is_channel_id(&"a".repeat(65)));
    }

    #[test]
    fn unicode_parent_urls_do_not_panic_during_address_detection() {
        for address in [format!("€{}", "x".repeat(39)), format!("a€{}", "x".repeat(38))] {
            assert_eq!(address.len(), 42);
            let url = format!("token:{address}");
            let label = short_parent_url_label(&url);
            assert!(label.starts_with("token:"));
            assert!(label.ends_with('…'));
            assert_eq!(channel_ref_from_url(Some(&url)).unwrap().label, label);
        }
    }
}

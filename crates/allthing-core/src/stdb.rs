// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's realtime database on SpacetimeDB (the module in
//! `spacetime/spacetimedb`): where it is, and the attestation that lets a
//! reader's SpacetimeDB identity prove which Farcaster account signed in.
//!
//! After Sign In With Farcaster, the browser asks the Worker to attest its
//! SpacetimeDB identity (`POST /api/v1/auth/attest`). The Worker signs
//! `attestation_message(identity, fid, expiry)` with its own Ed25519 key,
//! the `ATTEST_KEY` secret, used for nothing else; the browser hands the
//! signature to the module's `verify_fid` reducer, which checks it against
//! the key's public half pinned in the module and marks the identity's FID
//! verified. The attestation names the identity, so it is useless to anyone
//! else, and lapses after `ATTEST_TTL`.

use serde::{Deserialize, Serialize};

/// The SpacetimeDB host Allthing's database is on.
pub const STDB_URI: &str = "wss://maincloud.spacetimedb.com";
/// Allthing's database on it.
pub const STDB_DATABASE: &str = "allthing";
/// What an attestation signs, before its fields, so its signature can't
/// stand for anything else. The module uses the same string.
pub const ATTEST_CONTEXT: &str = "allthing-spacetimedb-fid-v1";
/// How long an attestation is valid for, in seconds (the module refuses
/// any valid for more than an hour).
pub const ATTEST_TTL: u64 = 600;

/// What the Worker signs for `identity` signed in as `fid` until `expiry`
/// (Unix seconds).
pub fn attestation_message(identity: &str, fid: u64, expiry: u64) -> String {
    format!("{ATTEST_CONTEXT}|{identity}|{fid}|{expiry}")
}

/// Whether `s` is a SpacetimeDB identity as the module spells it: 64
/// lower-case hex digits, no `0x`.
pub fn is_identity(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `POST /api/v1/auth/attest`'s body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestRequest {
    /// The browser's SpacetimeDB identity.
    pub identity: String,
}

/// The Worker's attestation: what `verify_fid(fid, expiry, signature)`
/// takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    pub identity: String,
    pub fid: u64,
    pub expiry: u64,
    /// The Ed25519 signature of `attestation_message`, 128 hex digits.
    pub signature: String,
}

/// The PKCS #8 wrapping of a raw 32-byte Ed25519 private key (RFC 8410),
/// as WebCrypto imports it.
pub fn ed25519_pkcs8(seed: &[u8; 32]) -> [u8; 48] {
    const PREFIX: [u8; 16] =
        [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
    let mut out = [0; 48];
    out[..16].copy_from_slice(&PREFIX);
    out[16..].copy_from_slice(seed);
    out
}

/// The `ATTEST_KEY` secret: 64 hex digits (32 bytes), with or without `0x`.
pub fn parse_attest_key(secret: &str) -> Option<[u8; 32]> {
    let hex = secret.trim();
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_matches_the_modules_test_vector() {
        // spacetime/spacetimedb/src/lib.rs `attestations` signs this very
        // string with RFC 8032's first test key.
        assert_eq!(
            attestation_message("c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15", 3, 1_800_000_000),
            "allthing-spacetimedb-fid-v1|c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15|3|1800000000"
        );
    }

    #[test]
    fn identities() {
        assert!(is_identity("c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15"));
        assert!(!is_identity("0xc200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15"));
        assert!(!is_identity("C200431F50CF410304546489E2600C8446408F1AA727C91D86BC00AEF6724D15"));
        assert!(!is_identity("c200"));
        assert!(!is_identity(&"g".repeat(64)));
    }

    #[test]
    fn keys() {
        let hex = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
        let key = parse_attest_key(hex).unwrap();
        assert_eq!(key[0], 0x9d);
        assert_eq!(parse_attest_key(&format!(" 0x{hex}\n")), Some(key));
        assert_eq!(parse_attest_key(&hex[2..]), None);
        assert_eq!(parse_attest_key(&hex.replace('9', "z")), None);
        assert_eq!(parse_attest_key("é".repeat(32).as_str()), None);
        let der = ed25519_pkcs8(&key);
        assert_eq!(der[..2], [0x30, 0x2e]);
        assert_eq!(der[16..], key);
    }
}

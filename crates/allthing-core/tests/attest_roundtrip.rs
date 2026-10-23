// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Worker's FID attestation, signed the way `allthing-worker/src/attest.rs`
//! signs it, verifies in the SpacetimeDB module's own check: this includes
//! the module's `spacetime/spacetimedb/src/attest.rs` itself, so the message,
//! its encoding, the expiry and the key formats can't drift apart.
//!
//! The keys here are throwaway test keys, never Allthing's.

#[path = "../../../spacetime/spacetimedb/src/attest.rs"]
#[allow(dead_code)]
mod module_attest;

use allthing_core::stdb::{
    ATTEST_CONTEXT, ATTEST_TTL, Attestation, attestation_message, ed25519_pkcs8, parse_attest_key,
};
use ed25519_dalek::{Signer, SigningKey};

const IDENTITY: &str = "c200431f50cf410304546489e2600c8446408f1aa727c91d86bc00aef6724d15";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// What the Worker does with the `ATTEST_KEY` secret's text at `now`, with
/// Ed25519 in place of the runtime's WebCrypto: the JSON it answers.
fn worker_attests(secret: &str, identity: &str, fid: u64, now: u64) -> String {
    let seed = parse_attest_key(secret).expect("the secret's format");
    // WebCrypto imports the seed wrapped in PKCS #8; the wrapping carries it unchanged.
    let der = ed25519_pkcs8(&seed);
    let seed: [u8; 32] = der[16..].try_into().unwrap();
    let expiry = now + ATTEST_TTL;
    let message = attestation_message(identity, fid, expiry);
    let signature = SigningKey::from_bytes(&seed).sign(message.as_bytes()).to_bytes();
    serde_json::to_string(&Attestation { identity: identity.into(), fid, expiry, signature: hex(&signature) }).unwrap()
}

#[test]
fn the_workers_attestation_verifies_in_the_module() {
    // A throwaway key, written the way `scripts/attest-key.mjs` prints the secret.
    let seed = [0x42u8; 32];
    let secret = hex(&seed);
    let public = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    let now = 1_800_000_000;

    // The browser reads the JSON and calls verify_fid(fid, expiry, signature bytes).
    let a: Attestation = serde_json::from_str(&worker_attests(&secret, IDENTITY, 3, now)).unwrap();
    assert_eq!(a.signature.len(), 128);
    let signature = unhex(&a.signature);
    assert_eq!(module_attest::check_attestation(&public, &a.identity, a.fid, a.expiry, now, &signature), Ok(()));
    // Still good just before it lapses; refused once it has.
    let last = a.expiry - 1;
    assert_eq!(module_attest::check_attestation(&public, IDENTITY, 3, a.expiry, last, &signature), Ok(()));
    assert!(module_attest::check_attestation(&public, IDENTITY, 3, a.expiry, a.expiry, &signature).is_err());
    // Not for another identity or FID, nor under another key.
    let other = IDENTITY.replace("c200", "c201");
    assert!(module_attest::check_attestation(&public, &other, 3, a.expiry, now, &signature).is_err());
    assert!(module_attest::check_attestation(&public, IDENTITY, 4, a.expiry, now, &signature).is_err());
    let stranger = SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes();
    assert!(module_attest::check_attestation(&stranger, IDENTITY, 3, a.expiry, now, &signature).is_err());
    // The secret as Wrangler may hand it over: `0x`, a trailing newline.
    let b: Attestation = serde_json::from_str(&worker_attests(&format!("0x{secret}\n"), IDENTITY, 3, now)).unwrap();
    assert_eq!(b, a);
}

#[test]
fn the_placeholder_key_refuses_clearly() {
    let a: Attestation = serde_json::from_str(&worker_attests(&hex(&[0x42; 32]), IDENTITY, 3, 0)).unwrap();
    let refused = module_attest::check_attestation(&[0; 32], IDENTITY, 3, a.expiry, 0, &unhex(&a.signature));
    assert_eq!(refused, Err(module_attest::NOT_SET_UP.to_string()));
    assert!(module_attest::NOT_SET_UP.contains("ATTEST_PUBLIC_KEY"));
}

#[test]
fn the_constants_agree() {
    assert_eq!(ATTEST_CONTEXT, module_attest::ATTEST_CONTEXT);
    const { assert!(ATTEST_TTL <= module_attest::ATTEST_MAX_TTL_SECS) };
    assert_eq!(attestation_message(IDENTITY, 3, 9), module_attest::attestation_message(IDENTITY, 3, 9));
}

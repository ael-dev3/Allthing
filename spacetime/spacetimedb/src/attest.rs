// SPDX-License-Identifier: AGPL-3.0-or-later
//! Checking Allthing's FID attestations: pure logic, with no SpacetimeDB
//! types, so Allthing's own tests (`crates/allthing-core/tests/attest_roundtrip.rs`)
//! include this very file and verify what the Worker's code path signs, and
//! the two formats can't drift apart.

/// The longest an attestation may be valid for, from now. The Worker signs
/// them for 10 minutes; a longer one is refused, so a leaked attestation is
/// worth little.
pub const ATTEST_MAX_TTL_SECS: u64 = 3600;
/// What an attestation signs, so its signature can't stand for anything else
/// the key might ever sign. Allthing's Worker builds the same string
/// (`allthing_core::stdb::attestation_message`).
pub const ATTEST_CONTEXT: &str = "allthing-spacetimedb-fid-v1";

/// `verify_fid`'s refusal while `ATTEST_PUBLIC_KEY` is the placeholder.
pub const NOT_SET_UP: &str = "FID verification isn't set up on this module yet: ATTEST_PUBLIC_KEY \
     (spacetime/spacetimedb/src/attest_key.rs) is still the all-zero placeholder";

/// Whether attestations are checked: the pinned key isn't the all-zero
/// placeholder.
pub fn attestations_on(key: &[u8; 32]) -> bool {
    key.iter().any(|b| *b != 0)
}

/// What Allthing's attestation key signs for `identity` (lower-case hex,
/// big-endian, no `0x`: `Identity::to_hex`) signed in as `fid` until
/// `expiry` (Unix seconds).
pub fn attestation_message(identity: &str, fid: u64, expiry: u64) -> String {
    format!("{ATTEST_CONTEXT}|{identity}|{fid}|{expiry}")
}

/// Check an attestation at `now` (Unix seconds): the key is pinned, the FID
/// is real, it hasn't expired and isn't valid for too long, and the
/// signature is the key's over `attestation_message`.
pub fn check_attestation(
    key: &[u8; 32],
    identity: &str,
    fid: u64,
    expiry: u64,
    now: u64,
    signature: &[u8],
) -> Result<(), String> {
    use ed25519_dalek::{Signature, VerifyingKey};
    if !attestations_on(key) {
        return Err(NOT_SET_UP.into());
    }
    if fid == 0 {
        return Err("bad fid".into());
    }
    if expiry <= now {
        return Err("attestation expired".into());
    }
    if expiry > now + ATTEST_MAX_TTL_SECS {
        return Err("attestation valid for too long".into());
    }
    let signature = Signature::from_slice(signature).map_err(|_| "attestation signature must be 64 bytes")?;
    let key = VerifyingKey::from_bytes(key).map_err(|_| "bad attestation key")?;
    key.verify_strict(attestation_message(identity, fid, expiry).as_bytes(), &signature)
        .map_err(|_| "attestation signature doesn't verify".into())
}

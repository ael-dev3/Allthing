// SPDX-License-Identifier: AGPL-3.0-or-later
//! The one place the module pins Allthing's FID attestation key.

/// The PUBLIC half of the Worker's `ATTEST_KEY` secret (Ed25519, 32 bytes).
///
/// All zeros is the placeholder: until it is replaced, `verify_fid` refuses
/// with "FID verification isn't set up on this module yet", and unverified
/// FIDs may still speak, as before.
///
/// To fill it (docs/DEPLOY.md#secrets, spacetime/HANDOFF.md): run
/// `node scripts/attest-key.mjs`, set its secret with
/// `npx wrangler secret put ATTEST_KEY`, replace this line with the
/// `pub const ATTEST_PUBLIC_KEY ...` line it prints (the public key only,
/// never the secret), commit, and republish the module.
pub const ATTEST_PUBLIC_KEY: [u8; 32] = [0; 32];

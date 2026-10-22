// SPDX-License-Identifier: AGPL-3.0-or-later
//! The public half of Allthing's FID attestation key, whose private half is
//! the Worker's `ATTEST_KEY` secret (docs/DEPLOY.md#secrets). All zeros
//! means not set up: `verify_fid` refuses, and unverified FIDs may still
//! speak, as before. `node scripts/attest-key.mjs` makes a key pair, gives
//! the private half to `wrangler secret put ATTEST_KEY` and writes the
//! public half here; the module must then be republished.

/// Allthing's attestation key (Ed25519, 32 bytes).
pub const ATTEST_PUBLIC_KEY: [u8; 32] = [0; 32];

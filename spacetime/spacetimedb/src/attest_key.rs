// SPDX-License-Identifier: AGPL-3.0-or-later
//! The public half of Allthing's FID attestation key, whose private half is
//! the Worker's `ATTEST_KEY` secret (docs/DEPLOY.md#secrets). All zeros
//! means not set up: `verify_fid` refuses, and unverified FIDs may still
//! speak, as before. `cargo xtask attest-key` makes a key pair, gives
//! the private half to `wrangler secret put ATTEST_KEY` and writes the
//! public half here; the module must then be republished.

/// Allthing's attestation key (Ed25519, 32 bytes).
pub const ATTEST_PUBLIC_KEY: [u8; 32] = [
    0x5d, 0xf2, 0x41, 0xb6, 0x51, 0x72, 0x35, 0xa5, 0x21, 0x76, 0x97, 0x88, 0x54, 0x1a, 0x5c, 0xb6, 0x36, 0x9b, 0xb3,
    0xde, 0xb4, 0x53, 0x67, 0x34, 0xb5, 0xdb, 0x8b, 0x5c, 0xb2, 0x3d, 0x87, 0xcb,
];

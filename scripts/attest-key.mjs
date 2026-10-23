#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Makes Allthing's FID attestation key pair (Ed25519) for Messages on
// SpacetimeDB (docs/DEPLOY.md#secrets, ATTEST_KEY), locally, and prints:
//   - the Worker secret ATTEST_KEY: the 32-byte private seed as 64 hex
//     digits, which `allthing_core::stdb::parse_attest_key` reads;
//   - the public half, as the line that replaces the placeholder
//     `pub const ATTEST_PUBLIC_KEY` in spacetime/spacetimedb/src/attest_key.rs;
//   - the commands to set the secret, edit the module and republish.
// It writes nothing to disk. Run on the owner's own machine only, and never
// paste the secret anywhere but Wrangler's prompt.
//
//   node scripts/attest-key.mjs                  # print all of the above
//   node scripts/attest-key.mjs | npx wrangler secret put ATTEST_KEY
//                                                # piped: only the secret goes
//                                                # to stdout, the rest to stderr
//   node scripts/attest-key.mjs --check <hex>    # the module line for a public
//                                                # key (e.g. /api/v1/status's)
//   node scripts/attest-key.mjs --self-test      # sign and verify with a
//                                                # throwaway key; prints no key
import { createPrivateKey, createPublicKey, generateKeyPairSync, sign, verify } from "node:crypto";

const FILE = "spacetime/spacetimedb/src/attest_key.rs";
const CONTEXT = "allthing-spacetimedb-fid-v1"; // allthing_core::stdb::ATTEST_CONTEXT

const fail = (msg) => {
  process.stderr.write(`${msg}\n`);
  process.exit(1);
};

/** The module's constant, for a 32-byte public key. */
function moduleLine(pub) {
  const bytes = [...pub].map((b) => `0x${b.toString(16).padStart(2, "0")}`).join(", ");
  return `pub const ATTEST_PUBLIC_KEY: [u8; 32] = [${bytes}];`;
}

function parseHex32(s, what) {
  const hex = (s ?? "").trim().replace(/^0x/, "");
  if (!/^[0-9a-fA-F]{64}$/.test(hex)) fail(`${what} must be 64 hex digits`);
  return Buffer.from(hex, "hex");
}

/** A key pair from a 32-byte seed, as the Worker imports it (PKCS #8, RFC 8410). */
function fromSeed(seed) {
  const der = Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), seed]);
  const privateKey = createPrivateKey({ key: der, format: "der", type: "pkcs8" });
  const pub = Buffer.from(createPublicKey(privateKey).export({ format: "jwk" }).x, "base64url");
  return { privateKey, pub };
}

const args = process.argv.slice(2);

if (args[0] === "--check") {
  const pub = parseHex32(args[1], "the public key");
  process.stdout.write(`${moduleLine(pub)}\n`);
  process.exit(0);
}

if (args[0] === "--self-test") {
  const { privateKey } = generateKeyPairSync("ed25519");
  const seed = Buffer.from(privateKey.export({ format: "jwk" }).d, "base64url");
  const { privateKey: imported, pub } = fromSeed(parseHex32(seed.toString("hex"), "seed"));
  const message = Buffer.from(`${CONTEXT}|${"ab".repeat(32)}|3|1800000000`);
  const signature = sign(null, message, imported);
  const publicKey = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: pub.toString("base64url") }, format: "jwk" });
  const ok = signature.length === 64 && verify(null, message, publicKey, signature);
  const line = moduleLine(pub);
  const back = Buffer.from(line.match(/0x[0-9a-f]{2}/g).map((h) => parseInt(h, 16)));
  if (!ok || !back.equals(pub)) fail("self-test FAILED");
  process.stdout.write("self-test ok: seed -> PKCS #8 -> signature verifies under the printed public key\n");
  process.exit(0);
}

if (args.length) fail("usage: node scripts/attest-key.mjs [--check <public key hex> | --self-test]");

const { publicKey, privateKey } = generateKeyPairSync("ed25519");
const seed = Buffer.from(privateKey.export({ format: "jwk" }).d, "base64url");
const pub = Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url");
if (seed.length !== 32 || pub.length !== 32) fail("unexpected Ed25519 key length");
if (!fromSeed(seed).pub.equals(pub)) fail("the seed doesn't give back its public key");

const piped = !process.stdout.isTTY;
const steps = `
ATTEST_KEY public key (safe to share; /api/v1/status shows it as spacetime.public_key):
  ${pub.toString("hex")}

Steps (docs/DEPLOY.md#secrets):
  1. Set the Worker secret${piped ? " (piped: already going to Wrangler)" : ", pasting the secret above at the prompt"}:
       npx wrangler secret put ATTEST_KEY
  2. In ${FILE}, replace the placeholder line
       pub const ATTEST_PUBLIC_KEY: [u8; 32] = [0; 32];
     with
       ${moduleLine(pub)}
     then: (cd spacetime/spacetimedb && cargo fmt) && sh spacetime/check.sh
  3. Commit that file, then republish the module:
       cd spacetime && spacetime publish -s maincloud allthing -y
  4. Confirm: https://allthing.social/api/v1/status shows "spacetime": {"attest": true,
     "public_key": "${pub.toString("hex")}"} (the key above; never the secret).
`;

if (piped) {
  process.stderr.write(steps);
  process.stdout.write(seed.toString("hex"));
} else {
  process.stdout.write(
    `ATTEST_KEY secret (private: paste only into Wrangler's prompt, never commit, chat or log it):\n  ${seed.toString("hex")}\n${steps}`,
  );
}

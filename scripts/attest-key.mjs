#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Makes Allthing's FID attestation key (Ed25519) for SpacetimeDB messages
// (docs/DEPLOY.md#secrets, ATTEST_KEY). Run it from the repository's root,
// piping it straight into Wrangler, so the private key goes only to
// Cloudflare and is never shown, saved or pasted:
//
//   node scripts/attest-key.mjs | npx wrangler secret put ATTEST_KEY
//
// It writes the public half into the module
// (spacetime/spacetimedb/src/attest_key.rs) and says it on stderr; commit
// that file and republish the module. Running it again makes a new key:
// the old one stops verifying once the module is republished with the new.
import { generateKeyPairSync } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

// Refuse before making anything if the key would land on a screen.
if (process.stdout.isTTY) {
  process.stderr.write("Not printing the private key to a terminal: pipe this into `npx wrangler secret put ATTEST_KEY`.\n");
  process.exit(1);
}

const FILE = "spacetime/spacetimedb/src/attest_key.rs";
const { publicKey, privateKey } = generateKeyPairSync("ed25519");
const seed = Buffer.from(privateKey.export({ format: "jwk" }).d, "base64url");
const pub = Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url");
if (seed.length !== 32 || pub.length !== 32) throw new Error("unexpected Ed25519 key length");

const source = readFileSync(FILE, "utf8");
const bytes = [...pub].map((b) => `0x${b.toString(16).padStart(2, "0")}`).join(", ");
const next = source.replace(/pub const ATTEST_PUBLIC_KEY: \[u8; 32\] = [^;]+;/, `pub const ATTEST_PUBLIC_KEY: [u8; 32] = [${bytes}];`);
if (next === source && !source.includes(bytes)) throw new Error(`no ATTEST_PUBLIC_KEY in ${FILE}`);
writeFileSync(FILE, next);

process.stderr.write(`ATTEST_KEY public half: ${pub.toString("hex")}\nwritten to ${FILE}; run cargo fmt in spacetime/spacetimedb, commit it and republish the module.\n`);
process.stdout.write(seed.toString("hex"));

// SPDX-License-Identifier: AGPL-3.0-or-later
// The end-to-end encryption's pinned vectors (crates/allthing-core/src/e2e.rs,
// tests/fixtures/e2e), recomputed apart from the Rust code: HKDF-SHA256 and
// ChaCha20-Poly1305 from Node's OpenSSL, X25519 too, and XChaCha20 built
// from HChaCha20 as draft-irtf-cfrg-xchacha-03 says, checked against its
// test vector first.
import assert from "node:assert/strict";
import { createCipheriv, createPrivateKey, createPublicKey, diffieHellman, hkdfSync } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const fixture = (name) => readFileSync(new URL(`../crates/allthing-core/tests/fixtures/e2e/${name}`, import.meta.url), "utf8").trim();

// RFC 7748 §6.1.
const A_PRIVATE = "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a";
const A_PUB = "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a";
const B_PRIVATE = "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb";
const B_PUB = "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f";
const A_ID = "c200aaaa00000000000000000000000000000000000000000000000000000001";
const B_ID = "c200bbbb00000000000000000000000000000000000000000000000000000002";

const x25519 = (hex) => createPrivateKey({ key: Buffer.concat([Buffer.from("302e020100300506032b656e04220420", "hex"), Buffer.from(hex, "hex")]), format: "der", type: "pkcs8" });

function hchacha20(key, nonce16) {
  const s = new Uint32Array(16);
  [0x61707865, 0x3320646e, 0x79622d32, 0x6b206574].forEach((c, i) => (s[i] = c));
  for (let i = 0; i < 8; i++) s[4 + i] = key.readUInt32LE(4 * i);
  for (let i = 0; i < 4; i++) s[12 + i] = nonce16.readUInt32LE(4 * i);
  const rot = (x, b) => (x << b) | (x >>> (32 - b));
  const q = (a, b, c, d) => {
    s[a] += s[b]; s[d] = rot(s[d] ^ s[a], 16); s[c] += s[d]; s[b] = rot(s[b] ^ s[c], 12);
    s[a] += s[b]; s[d] = rot(s[d] ^ s[a], 8); s[c] += s[d]; s[b] = rot(s[b] ^ s[c], 7);
  };
  for (let i = 0; i < 10; i++) {
    q(0, 4, 8, 12); q(1, 5, 9, 13); q(2, 6, 10, 14); q(3, 7, 11, 15);
    q(0, 5, 10, 15); q(1, 6, 11, 12); q(2, 7, 8, 13); q(3, 4, 9, 14);
  }
  const out = Buffer.alloc(32);
  [0, 1, 2, 3, 12, 13, 14, 15].forEach((j, i) => out.writeUInt32LE(s[j] >>> 0, 4 * i));
  return out;
}

test("HChaCha20 matches draft-irtf-cfrg-xchacha-03 §2.2.1", () => {
  const key = Buffer.from("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f", "hex");
  const nonce = Buffer.from("000000090000004a0000000031415927", "hex");
  assert.equal(hchacha20(key, nonce).toString("hex"), "82413b4227b27bfed30e42508a877d73a0f9e4d58a74a853c12ec41326d3ecdc");
});

test("the pinned message key and ciphertext are what the scheme says", () => {
  const shared = diffieHellman({ privateKey: x25519(A_PRIVATE), publicKey: createPublicKey(x25519(B_PRIVATE)) });
  assert.equal(createPublicKey(x25519(A_PRIVATE)).export({ format: "jwk" }).x, Buffer.from(A_PUB, "hex").toString("base64url"));
  assert.equal(shared.toString("hex"), "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
  // Alice is version 1, Bob version 3; the lower identity comes first.
  const info = `allthing-dm-v1|key|${A_ID}:1:${A_PUB}|${B_ID}:3:${B_PUB}`;
  const key = Buffer.from(hkdfSync("sha256", shared, Buffer.from("allthing-dm-v1"), Buffer.from(info), 32));
  assert.equal(key.toString("hex"), fixture("key.hex"));

  const nonce = Buffer.alloc(24, 7);
  const text = Buffer.from("gm 🐸, end to end");
  const envelope = Buffer.alloc(32);
  envelope[0] = 1;
  envelope.writeUInt16BE(text.length, 1);
  text.copy(envelope, 3);
  const aad = Buffer.from(`allthing-dm-v1|from|${A_ID}:1:${A_PUB}|to|${B_ID}:3:${B_PUB}`);
  const cipher = createCipheriv("chacha20-poly1305", hchacha20(key, nonce.subarray(0, 16)), Buffer.concat([Buffer.alloc(4), nonce.subarray(16)]), { authTagLength: 16 });
  cipher.setAAD(aad, { plaintextLength: envelope.length });
  const sealed = Buffer.concat([cipher.update(envelope), cipher.final(), cipher.getAuthTag()]);
  assert.equal(sealed.toString("hex"), fixture("sealed.hex"));
});

// SPDX-License-Identifier: AGPL-3.0-or-later
import { createPublicKey, verify } from "node:crypto";
import { expect, type Page } from "@playwright/test";

/*
 * Farcaster messages as the browser hands them to the node, decoded, and
 * the reader's key as the browser keeps it: for the specs of liking,
 * recasting and casting from Allthing.
 */

export type Fields = Map<number, (bigint | Buffer)[]>;

/** A protobuf message's varint and length-delimited fields, by number. */
export function fields(bytes: Buffer): Fields {
  const out: Fields = new Map();
  let at = 0;
  const varint = () => {
    let value = 0n;
    for (let shift = 0n; ; shift += 7n) {
      const byte = bytes[at++];
      value |= BigInt(byte & 0x7f) << shift;
      if (byte < 0x80) return value;
    }
  };
  while (at < bytes.length) {
    const tag = Number(varint());
    const [field, wire] = [tag >> 3, tag & 7];
    if (wire !== 0 && wire !== 2) throw new Error(`wire type ${wire}`);
    let value: bigint | Buffer;
    if (wire === 0) {
      value = varint();
    } else {
      const length = Number(varint());
      value = bytes.subarray(at, at + length);
      at += length;
    }
    out.set(field, [...(out.get(field) ?? []), value]);
  }
  return out;
}

export const num = (f: Fields, n: number) => Number((f.get(n)?.[0] as bigint | undefined) ?? 0n);
export const buf = (f: Fields, n: number) => (f.get(n)?.[0] as Buffer | undefined) ?? Buffer.alloc(0);
const bufs = (f: Fields, n: number) => (f.get(n) ?? []) as Buffer[];
const hex = (b: Buffer) => `0x${b.toString("hex")}`;

/** Varints packed in one field (proto3's repeated numbers). */
export function packed(bytes: Buffer): number[] {
  const out: number[] = [];
  let at = 0;
  while (at < bytes.length) {
    let value = 0;
    for (let shift = 0; ; shift += 7) {
      const byte = bytes[at++];
      value += (byte & 0x7f) * 2 ** shift;
      if (byte < 0x80) break;
    }
    out.push(value);
  }
  return out;
}

export type Envelope = { hash: string; signer: string; type: number; fid: number; at: number; network: number; body: Fields };

/** A message the node was given: its signature checked, its data read. */
export function envelope(body: Buffer): Envelope {
  const message = fields(body);
  const [hash, signature, signer, data] = [buf(message, 2), buf(message, 4), buf(message, 6), buf(message, 7)];
  expect(hash).toHaveLength(20);
  expect([num(message, 3), num(message, 5)]).toEqual([1, 1]); // BLAKE3, Ed25519
  const key = createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), signer]), format: "der", type: "spki" });
  expect(verify(null, hash, key, signature)).toBe(true);
  const d = fields(data);
  const type = num(d, 1);
  // MessageData's body field for each message type.
  const bodyField = { 1: 5, 2: 6, 3: 7, 4: 7, 5: 14, 6: 14 }[type] ?? 0;
  return { hash: hex(hash), signer: hex(signer), type, fid: num(d, 2), at: num(d, 3), network: num(d, 4), body: fields(buf(d, bodyField)) };
}

const castId = (f: Fields) => ({ fid: num(f, 1), hash: hex(buf(f, 2)) });

/** A like or recast (type 3, or 4 to undo). */
export function reaction(e: Envelope) {
  return { reaction: num(e.body, 1), target: castId(fields(buf(e.body, 2))) };
}

/** A cast (type 1): its text, mentions, embeds, parent and cast type. */
export function cast(e: Envelope) {
  const embeds = bufs(e.body, 6).map((raw) => {
    const embed = fields(raw);
    return embed.has(1) ? { url: buf(embed, 1).toString("utf8") } : { cast: castId(fields(buf(embed, 2))) };
  });
  return {
    text: buf(e.body, 4).toString("utf8"),
    mentions: packed(buf(e.body, 2)),
    positions: packed(buf(e.body, 5)),
    embeds,
    parent: e.body.has(3) ? { cast: castId(fields(buf(e.body, 3))) } : e.body.has(7) ? { url: buf(e.body, 7).toString("utf8") } : null,
    type: num(e.body, 8),
  };
}

/** Signed in as `user`, as the Worker's user cookie says. */
export async function signIn(page: Page, user: object) {
  await page.addInitScript((user) => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify(user))}; Path=/; Secure; SameSite=Lax`;
  }, user);
}

/** The key this browser keeps for `fid`, as IndexedDB holds it. */
export function kept(page: Page, fid: number) {
  return page.evaluate(
    (fid) =>
      new Promise<null | { key: string; extractable: boolean; algorithm: string; approved: boolean; token: string | null }>((resolve, reject) => {
        const open = indexedDB.open("allthing-keys", 1);
        open.onupgradeneeded = () => open.result.createObjectStore("keys", { keyPath: "fid" });
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          const get = open.result.transaction("keys").objectStore("keys").get(fid);
          get.onsuccess = () => {
            const r = get.result;
            open.result.close();
            resolve(r ? { key: r.key, extractable: r.private.extractable, algorithm: r.private.algorithm.name, approved: Boolean(r.approvedAt), token: r.token ?? null } : null);
          };
        };
      }),
    fid,
  );
}

/** An approved key for `fid`, as if the reader had approved one `ago` ms back; its public half. */
export function approvedKey(page: Page, fid: number, ago: number) {
  return page.evaluate(
    async ({ fid, approvedAt }) => {
      const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
      const raw = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
      const key = `0x${[...raw].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
      await new Promise((resolve, reject) => {
        const open = indexedDB.open("allthing-keys", 1);
        open.onupgradeneeded = () => open.result.createObjectStore("keys", { keyPath: "fid" });
        open.onerror = () => reject(open.error);
        open.onsuccess = () => {
          const tx = open.result.transaction("keys", "readwrite");
          tx.objectStore("keys").put({ fid, key, private: pair.privateKey, approvedAt });
          tx.oncomplete = () => {
            open.result.close();
            resolve(null);
          };
        };
      });
      return key;
    },
    { fid, approvedAt: Date.now() - ago },
  );
}

/** A follow (type 5), or an unfollow (6): its link type and target. */
export function link(e: Envelope) {
  return { type: buf(e.body, 1).toString("utf8"), target: num(e.body, 3) };
}

/** A deleted cast (type 2): the hash it takes back. */
export function removed(e: Envelope) {
  return hex(buf(e.body, 1));
}

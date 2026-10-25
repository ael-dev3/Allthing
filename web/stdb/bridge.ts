// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The bridge between Allthing's app (Rust, WebAssembly) and its SpacetimeDB
// database, through the official TypeScript SDK (`spacetimedb` on npm) and
// the bindings `spacetime generate` makes from the module
// (spacetime/spacetimedb). SpacetimeDB's Rust client SDK builds for the
// browser too, but under the Business Source License, which Allthing's
// AGPL and its license audit don't take; the TypeScript SDK is ISC.
//
// The native xtask bundles this through esbuild into one file, `assets/stdb-<hash>.js`,
// which the app adds once for Messages or eligible signed-in notification
// read-state sync. The separate connections share the script. It sets
// `window.allthingStdb`, and the app talks to it in JSON strings, so the
// app's own module needs no glue for it.
//
// It holds no key but the SpacetimeDB token the app hands it, does no
// encryption (the app does, in Rust, with keys only the browser has) and
// sends nothing but what the app asks: reducer calls and subscriptions.

import "./csp-codec";
import { Identity, setGlobalLogLevel } from "spacetimedb";
import { DbConnection } from "./module_bindings";

type Emit = (event: string) => void;

const hex = (bytes: Uint8Array) => Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
const unhex = (s: string) => Uint8Array.from(s.match(/../g) ?? [], (h) => parseInt(h, 16));
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Any = any;

/** A row as the app reads it: identities and bytes as hex, numbers as numbers, times in microseconds. */
function plain(row: Any): Any {
  const out: Any = {};
  for (const [key, value] of Object.entries(row)) {
    const name = key.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
    const v = value as Any;
    if (typeof v === "bigint") out[name] = Number(v);
    else if (v instanceof Uint8Array) out[name] = hex(v);
    else if (v && typeof v.toHexString === "function") out[name] = v.toHexString();
    else if (v && v.__timestamp_micros_since_unix_epoch__ !== undefined) out[name] = Number(v.__timestamp_micros_since_unix_epoch__);
    else out[name] = v;
  }
  return out;
}

/** The tables and views the app follows, by the module's names. */
const TABLES: Record<string, string> = {
  user: "user",
  userKey: "user_key",
  room: "room",
  chatMessage: "chat_message",
  typing: "typing",
  myDmInbox: "my_dm_inbox",
  myDmOutbox: "my_dm_outbox",
  myNotificationState: "my_notification_state",
};

function table(conn: Any, accessor: string): Any {
  return conn.db[accessor] ?? conn.db[TABLES[accessor]];
}

function reducer(conn: Any, camel: string): (args: Any) => Promise<void> {
  const snake = camel.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
  const fn = conn.reducers[camel] ?? conn.reducers[snake];
  if (typeof fn !== "function") throw new Error(`no reducer ${camel}`);
  return fn.bind(conn.reducers);
}

/** Await a reducer call, as `{"ok":true}` or `{"ok":false,"error":…}`. */
async function settle(call: () => Promise<void>): Promise<string> {
  try {
    await call();
    return JSON.stringify({ ok: true });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return JSON.stringify({ ok: false, error: message });
  }
}

class Bridge {
  conn: Any;
  emit: Emit;
  /** Resolves with `{"identity","token"}` once connected; rejects if it can't. */
  connected!: Promise<string>;

  constructor(conn: Any, emit: Emit) {
    this.conn = conn;
    this.emit = emit;
    for (const [accessor, name] of Object.entries(TABLES)) {
      const t = table(conn, accessor);
      if (!t) continue;
        t.onInsert((_ctx: Any, row: Any) => this.emit(JSON.stringify({ t: "insert", table: name, row: plain(row) })));
        t.onDelete((_ctx: Any, row: Any) => this.emit(JSON.stringify({ t: "delete", table: name, row: plain(row) })));
      if (typeof t.onUpdate === "function") {
        t.onUpdate((_ctx: Any, old: Any, row: Any) =>
          this.emit(JSON.stringify({ t: "update", table: name, old: plain(old), row: plain(row) })),
        );
      }
    }
  }

  /** Follow these SQL queries: resolves once they are applied (every row has come as `insert`). */
  subscribe(queriesJson: string): Promise<void> {
    const queries = JSON.parse(queriesJson) as string[];
    return new Promise((resolve, reject) => {
      this.conn
        .subscriptionBuilder()
        .onApplied(() => resolve())
        .onError((_ctx: Any, error: Any) => reject(new Error(String(error?.message ?? error ?? "subscription failed"))))
        .subscribe(queries);
    });
  }

  register(fid: number) {
    return settle(() => reducer(this.conn, "register")({ fid: BigInt(fid) }));
  }

  verifyFid(fid: number, expiry: number, signatureHex: string) {
    return settle(() =>
      reducer(this.conn, "verifyFid")({ fid: BigInt(fid), expiry: BigInt(expiry), signature: unhex(signatureHex) }),
    );
  }

  verifyNotificationFid(fid: number, expiry: number, signatureHex: string) {
    return settle(() =>
      reducer(this.conn, "verifyNotificationFid")({ fid: BigInt(fid), expiry: BigInt(expiry), signature: unhex(signatureHex) }),
    );
  }

  markNotificationsSeen(seenAt: number) {
    return settle(() => reducer(this.conn, "markNotificationsSeen")({ seenAt: BigInt(seenAt) }));
  }

  setPublicKey(publicKeyHex: string) {
    return settle(() => reducer(this.conn, "setPublicKey")({ x25519Pub: unhex(publicKeyHex) }));
  }

  sendMessage(roomId: number, text: string) {
    return settle(() => reducer(this.conn, "sendMessage")({ roomId: BigInt(roomId), text }));
  }

  setTyping(roomId: number, typing: boolean) {
    return settle(() => reducer(this.conn, "setTyping")({ roomId: BigInt(roomId), typing }));
  }

  sendDm(recipientHex: string, senderKeyVersion: number, recipientKeyVersion: number, nonceHex: string, ciphertextHex: string) {
    const identity = Identity.fromString(recipientHex);
    return settle(() =>
      reducer(this.conn, "sendDm")({
        recipient: identity,
        senderKeyVersion,
        recipientKeyVersion,
        nonce: unhex(nonceHex),
        ciphertext: unhex(ciphertextHex),
      }),
    );
  }

  deleteDm(id: number) {
    return settle(() => reducer(this.conn, "deleteDm")({ id: BigInt(id) }));
  }

  disconnect() {
    // Release the caller's Wasm closure before an asynchronous close event.
    this.emit = () => {};
    this.conn.disconnect();
  }
}

/**
 * Connect to `database` at `uri` with the stored `token` (or none, for a
 * new identity). `emit` hears rows as JSON (`insert`, `delete`, `update`)
 * and `disconnected`; `connected` resolves with the identity and token.
 */
function connect(uri: string, database: string, token: string, emit: Emit): Bridge {
  let bridge: Bridge | undefined;
  let settle: { resolve: (v: string) => void; reject: (e: Error) => void } | undefined;
  const connected = new Promise<string>((resolve, reject) => (settle = { resolve, reject }));
  const conn = DbConnection.builder()
    .withUri(uri)
    .withDatabaseName(database)
    .withToken(token || undefined)
    .onConnect((_conn: Any, identity: Any, newToken: string) =>
      settle?.resolve(JSON.stringify({ identity: identity.toHexString(), token: newToken })),
    )
    .onConnectError((_ctx: Any, error: Any) => settle?.reject(new Error(String(error?.message ?? error))))
    .onDisconnect((_ctx: Any, error: Any) => {
      // A socket can close before InitialConnection arrives. Wake callers
      // awaiting connected immediately; rejecting an already settled promise
      // is harmless for established connections.
      settle?.reject(new Error(String(error?.message ?? error ?? "Disconnected from the database")));
      bridge?.emit(JSON.stringify({ t: "disconnected", error: error ? String(error?.message ?? error) : null }));
    })
    .build();
  bridge = new Bridge(conn, emit);
  bridge.connected = connected;
  return bridge;
}

// The SDK logs every connection at "info"; the console stays quiet.
setGlobalLogLevel("warn");
(globalThis as Any).allthingStdb = { connect, version: 1 };

// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, type Page, type Route } from "@playwright/test";
import { cast, envelope, link, reaction, removed, type Envelope } from "./farcaster";

/*
 * A stand-in for the node (Hypersnap) in the browser's requests. It takes
 * the messages the browser signs and checks each as Snapchain would: its
 * hash is BLAKE3 of its data (`envelope`), its signature is its signer's,
 * it is on mainnet, by the reader, signed with the key they approved, and
 * dated within ten minutes of now. Then it answers the browser's read-backs
 * (`reactionById`, `castById`, `linkById`) from what it has merged, as the
 * node's store would. What it does with each message is up to the spec: it
 * may merge it, take it and never merge it (as when Snapchain's validators
 * drop it), refuse it, be busy, or lose the answer on the way back.
 */

export const NODE = "https://haatz.quilibrium.com/v1";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const EPOCH = 1_609_459_200;

/** What the node does with a message it hasn't seen. */
export type Answer =
  /** Takes it (200), and has it at once. */
  | "merge"
  /** Takes it (200), and has it this many ms later. */
  | { mergeAfter: number }
  /** Takes it (200), and never has it: the validators dropped it. Handed in again, it is new to the node. */
  | "never"
  /** Takes and merges it, but the answer is lost on the way back. */
  | "lost"
  /** Busy: a 400 whose gRPC status is Unavailable, as Hypersnap answers. */
  | "unavailable"
  /** Down: a 503. */
  | "down"
  /** Doesn't know the key: removed in the Farcaster app. */
  | "unknown-key"
  /** Refuses it, in these words. */
  | { refuse: string };

export type StandInOptions = {
  /** The reader: every message must be theirs. */
  fid: number;
  /** The key the reader approved (`0x…`), once known: every message must be signed with it. */
  key?: () => string;
  /** What the node does with the nth new message (from 1); merges it by default. */
  answer?: (n: number, message: Envelope) => Answer;
  /** How long the node takes to answer a message, in ms. */
  slow?: number;
};

export type StandIn = {
  /** Every message handed in, in order (a message handed in again, again). */
  sent: Envelope[];
  /** The read-backs asked, as `castById?fid=…&hash=…` and so on. */
  reads: string[];
  /** What the node answered each message: its status, or 0 for none. */
  answers: number[];
  /** The node's store: merged messages, by hash. */
  merged: Map<string, Envelope>;
  /** What the store has from before: a reaction, a follow or a cast of someone's. */
  had: {
    reaction(fid: number, kind: "Like" | "Recast", target: { fid: number; hash: string }, hash?: string): void;
    follow(fid: number, target: number): void;
    cast(fid: number, hash: string): void;
  };
};

type Kept = { hash: string; fid: number; at: number; json: object };

export async function standIn(page: Page, options: StandInOptions): Promise<StandIn> {
  const sent: Envelope[] = [];
  const reads: string[] = [];
  const answers: number[] = [];
  const merged = new Map<string, Envelope>();
  // What the node has seen, by hash: merged, or on its way.
  const seen = new Set<string>();
  // The store's live adds: reactions by "fid kind target", follows by
  // "fid target", casts by hash.
  const reactions = new Map<string, Kept>();
  const follows = new Map<string, Kept>();
  const casts = new Map<string, Kept>();
  let fresh = 0;

  const reactionKey = (fid: number, kind: number, target: { hash: string }) => `${fid} ${kind} ${target.hash.toLowerCase()}`;
  const KINDS: Record<number, string> = { 1: "REACTION_TYPE_LIKE", 2: "REACTION_TYPE_RECAST" };
  const meta = (e: { hash: string; fid: number; at: number }) => ({ hash: e.hash, fid: e.fid, at: e.at });

  /** The store, after a message: as Snapchain's sets keep them, the later of two for the same thing winning. */
  function merge(e: Envelope) {
    merged.set(e.hash, e);
    const later = (kept: Kept | undefined) => !kept || kept.at <= e.at;
    if (e.type === 3 || e.type === 4) {
      const { reaction: kind, target } = reaction(e);
      const key = reactionKey(e.fid, kind, target);
      if (!later(reactions.get(key))) return;
      if (e.type === 4) return void reactions.delete(key);
      const data = { type: "MESSAGE_TYPE_REACTION_ADD", fid: e.fid, timestamp: e.at, network: "FARCASTER_NETWORK_MAINNET", reactionBody: { type: KINDS[kind], targetCastId: target } };
      reactions.set(key, { ...meta(e), json: { data, hash: e.hash } });
    } else if (e.type === 5 || e.type === 6) {
      const { target } = link(e);
      const key = `${e.fid} ${target}`;
      if (!later(follows.get(key))) return;
      if (e.type === 6) return void follows.delete(key);
      const data = { type: "MESSAGE_TYPE_LINK_ADD", fid: e.fid, timestamp: e.at, network: "FARCASTER_NETWORK_MAINNET", linkBody: { type: "follow", targetFid: target } };
      follows.set(key, { ...meta(e), json: { data, hash: e.hash } });
    } else if (e.type === 1) {
      const c = cast(e);
      const castAddBody = {
        text: c.text,
        mentions: c.mentions,
        mentionsPositions: c.positions,
        embeds: c.embeds.map((embed) => ("url" in embed ? { url: embed.url } : { castId: embed.cast })),
        parentCastId: c.parent && "cast" in c.parent ? c.parent.cast : null,
        parentUrl: c.parent && "url" in c.parent ? c.parent.url : null,
      };
      const data = { type: "MESSAGE_TYPE_CAST_ADD", fid: e.fid, timestamp: e.at, network: "FARCASTER_NETWORK_MAINNET", castAddBody };
      casts.set(e.hash, { ...meta(e), json: { data, hash: e.hash } });
    } else if (e.type === 2) {
      casts.delete(removed(e));
    }
  }

  const fail = (route: Route, error: string, detail: string) =>
    route.fulfill({ status: 400, headers: cors, json: { error, error_detail: `status: ${detail}, details: [], metadata: MetadataMap { headers: {} }` } });
  const missing = (route: Route, what: string) => fail(route, `Failed to get ${what}`, 'NotFound, message: "not found"');

  await page.route(`${NODE}/submitMessage`, async (route) => {
    if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers: cors });
    expect(route.request().headers()["content-type"]).toBe("application/octet-stream");
    const e = envelope(route.request().postDataBuffer()!);
    sent.push(e);
    expect([e.fid, e.network]).toEqual([options.fid, 1]);
    const key = options.key?.();
    if (key) expect(e.signer).toBe(key);
    expect(Math.abs(e.at - (Date.now() / 1000 - EPOCH))).toBeLessThan(600);
    if (options.slow) await new Promise((done) => setTimeout(done, options.slow));
    const answer = async (status: number, json?: object) => {
      answers.push(status);
      return route.fulfill({ status, headers: cors, json: json ?? {} });
    };
    if (seen.has(e.hash)) {
      const words = merged.has(e.hash) ? "message has already been merged" : "message already in the mempool";
      answers.push(400);
      return fail(route, "Failed to submit message", `InvalidArgument, message: "bad_request.duplicate/${words}"`);
    }
    const what = options.answer?.(++fresh, e) ?? "merge";
    if (what === "lost") {
      seen.add(e.hash);
      merge(e);
      answers.push(0);
      return route.abort("connectionreset");
    }
    if (what === "unavailable") {
      answers.push(400);
      return fail(route, "Failed to submit message", 'Unavailable, message: "unavailable/mempool channel is full"');
    }
    if (what === "down") return answer(503, { error: "unavailable" });
    if (what === "unknown-key") {
      answers.push(400);
      return fail(route, "Failed to submit message", 'InvalidArgument, message: "bad_request.validation_failure/invalid signer"');
    }
    if (typeof what === "object" && "refuse" in what) {
      answers.push(400);
      return fail(route, "Failed to submit message", `InvalidArgument, message: "bad_request.validation_failure/${what.refuse}"`);
    }
    // A message the validators drop isn't kept: a read node has no mempool
    // of its own, and takes it again.
    if (what !== "never") seen.add(e.hash);
    if (what === "merge") merge(e);
    else if (typeof what === "object" && "mergeAfter" in what) setTimeout(() => merge(e), what.mergeAfter);
    // The node answers with the message it took, as JSON.
    return answer(200, { hash: e.hash });
  });

  const read = (route: Route) => {
    const url = new URL(route.request().url());
    reads.push(`${url.pathname.split("/").pop()}${url.search}`);
    return url.searchParams;
  };
  await page.route(`${NODE}/reactionById?*`, (route) => {
    const q = read(route);
    const kind = { Like: 1, Recast: 2 }[q.get("reaction_type")!]!;
    const kept = reactions.get(reactionKey(Number(q.get("fid")), kind, { hash: q.get("target_hash")! }));
    return kept ? route.fulfill({ headers: cors, json: kept.json }) : missing(route, "reaction");
  });
  await page.route(`${NODE}/castById?*`, (route) => {
    const q = read(route);
    const kept = casts.get(q.get("hash")!.toLowerCase());
    return kept && kept.fid === Number(q.get("fid")) ? route.fulfill({ headers: cors, json: kept.json }) : missing(route, "cast");
  });
  await page.route(`${NODE}/linkById?*`, (route) => {
    const q = read(route);
    const kept = follows.get(`${q.get("fid")} ${q.get("target_fid")}`);
    return kept ? route.fulfill({ headers: cors, json: kept.json }) : missing(route, "link");
  });

  const before = (fid: number, json: object, hash: string) => ({ hash, fid, at: 0, json: { ...json, hash } });
  const had: StandIn["had"] = {
    reaction(fid, kind, target, hash = `0x${"be".repeat(20)}`) {
      const data = { type: "MESSAGE_TYPE_REACTION_ADD", fid, timestamp: 1, network: "FARCASTER_NETWORK_MAINNET", reactionBody: { type: `REACTION_TYPE_${kind.toUpperCase()}`, targetCastId: target } };
      reactions.set(reactionKey(fid, kind === "Like" ? 1 : 2, target), before(fid, { data }, hash));
    },
    follow(fid, target) {
      const data = { type: "MESSAGE_TYPE_LINK_ADD", fid, timestamp: 1, network: "FARCASTER_NETWORK_MAINNET", linkBody: { type: "follow", targetFid: target } };
      follows.set(`${fid} ${target}`, before(fid, { data }, `0x${"f0".repeat(20)}`));
    },
    cast(fid, hash) {
      const data = { type: "MESSAGE_TYPE_CAST_ADD", fid, timestamp: 1, network: "FARCASTER_NETWORK_MAINNET", castAddBody: { text: "", embeds: [], mentions: [], mentionsPositions: [] } };
      casts.set(hash.toLowerCase(), before(fid, { data }, hash.toLowerCase()));
    },
  };
  return { sent, reads, answers, merged, had };
}

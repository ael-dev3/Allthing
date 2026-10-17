import { describe, expect, it, vi } from "vitest";
import { createDataSource } from "@/lib/datasource";
import { MemorySource } from "@/lib/datasource/memory";
import { DataSourceError, hypersnapSource, neynarSource } from "@/lib/datasource/neynar";
import { normalizeCast, normalizeChannel, normalizeThread, type RawCast } from "@/lib/datasource/normalize";
import { collectCasts } from "@/lib/datasource/pool";
import type { Cast } from "@/lib/types";
import conversation from "./fixtures/hypersnap-conversation.json";
import legacyChannel from "./fixtures/hypersnap-channel-legacy.json";
import channel from "./fixtures/hypersnap-channel.json";
import trendingChannels from "./fixtures/hypersnap-channel-trending.json";
import trending from "./fixtures/hypersnap-feed-trending.json";
import userCasts from "./fixtures/hypersnap-user-casts.json";
import userSearch from "./fixtures/hypersnap-user-search.json";
import user from "./fixtures/hypersnap-user.json";

const FARCASTER_URL = "chain://eip155:7777777/erc721:0x4f86113fc3e9783cf3ec9a552cbb566716a57628";

/** A fetch stub that serves fixtures by path and records every URL it saw. */
function stubFetch(routes: Record<string, unknown | ((url: URL) => unknown)>) {
  const calls: URL[] = [];
  const fetch = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
    const url = new URL(input instanceof Request ? input.url : input);
    calls.push(url);
    const path = url.pathname.replace(/^\/v2\/farcaster\//, "");
    const route = routes[path];
    const body = typeof route === "function" ? route(url) : route;
    void init;
    if (body === undefined) return new Response(JSON.stringify({ message: "not found" }), { status: 404 });
    if (body instanceof Response) return body;
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  });
  return { fetch: fetch as unknown as typeof globalThis.fetch, calls, mock: fetch };
}

describe("normalize (live Hypersnap fixtures)", () => {
  const casts = trending.casts.map((raw) => normalizeCast(raw as RawCast));

  it("normalizes hashes, counts and channels", () => {
    const [swarm, prize] = casts;
    expect(swarm.hash).toBe("0x09eb33c944884cf1bbe2713ddcc87056fdd627b2");
    expect(swarm.channel).toEqual({ id: "farcaster", label: "farcaster" });
    expect(swarm.likes).toBe(184);
    expect(swarm.recasts).toBe(184);
    expect(swarm.replies).toBe(185);
    expect(prize.channel).toBeNull();
    expect(prize.author).toEqual({
      fid: 37,
      username: "balajis.eth",
      displayName: "balajis",
      pfpUrl: "https://github.com/balajis.png",
    });
  });

  it("classifies embeds", () => {
    const prize = casts[1];
    expect(prize.embeds[0]).toEqual({
      kind: "cast",
      hash: expect.stringMatching(/^0x[0-9a-f]{40}$/),
      fid: expect.any(Number),
    });
    const dwr = casts[2];
    expect(dwr.embeds[0].kind).toBe("image");
    const gm = casts.find((c) => c.channel?.id === "gmfarcaster");
    expect(gm?.embeds.length).toBeGreaterThan(1);
  });

  it("builds the reply tree and splices mentions at byte offsets", () => {
    const tree = normalizeThread(conversation.conversation.cast as RawCast);
    expect(tree.cast.hash).toBe("0xf454b8d3e5ee2c9252a0b7574ad4942e9ba502d2");
    expect(tree.children.length).toBe(5);

    const linda = tree.children.find((n) => n.cast.author.username === "linda");
    expect(linda?.cast.text).toBe("I love that, congrats @dylanb.eth!");
    expect(linda?.children[0].cast.author.username).toBe("darrylyeo");

    const dylan = tree.children.find((n) => n.cast.author.username === "dylanb.eth");
    const speck = dylan?.children[0];
    expect(speck?.cast.text).toContain("Well done, @balajis.eth and Team!");
    const tip = speck?.children[0].cast;
    expect(tip?.text).toMatch(/^Nothing held back! 🌴\n\n@specktacular tipped @dylanb\.eth 576 UCI/);
    expect(tip?.parentHash).toBe(speck?.cast.hash);
  });

  it("resolves Hypersnap's URL-keyed legacy channel to its real name", () => {
    expect(normalizeChannel(legacyChannel.channel)).toEqual({
      id: "farcaster",
      name: "farcaster",
      parentUrl: FARCASTER_URL,
      description: null,
      imageUrl: null,
      memberCount: 94568,
      createdAt: legacyChannel.channel.created_at,
    });
    expect(normalizeChannel(channel.channel)).toMatchObject({
      id: "gifart",
      name: "gifart",
      parentUrl: "https://warpcast.com/~/channel/gifart",
    });
    const token = normalizeChannel(trendingChannels.channels[2]);
    expect(token.id).toBe(trendingChannels.channels[2].parent_url);
    expect(token.name).toMatch(/^erc20:0x/);
  });

  it("reads Neynar's richer shapes too", () => {
    const neynarChannel = normalizeChannel({
      id: "farcaster",
      name: "Farcaster",
      url: FARCASTER_URL,
      parent_url: FARCASTER_URL,
      description: "Discussions about Farcaster",
      image_url: "https://example.com/fc.png",
      follower_count: 1000,
      created_at: 1689888729,
    });
    expect(neynarChannel).toMatchObject({ id: "farcaster", name: "Farcaster", memberCount: 1000 });
    expect(neynarChannel.createdAt).toBe("2023-07-20T21:32:09.000Z");

    const neynarCast = normalizeCast({
      hash: "0xABC",
      text: "hi @dwr",
      author: { fid: 1, username: "a" },
      channel: { id: "dev" },
      mentioned_profiles: [{ fid: 3, username: "dwr" }],
      mentioned_profiles_ranges: [{ start: 3, end: 7 }],
      embeds: [
        {
          url: "https://example.com/post",
          metadata: { content_type: "text/html", html: { ogImage: [{ url: "https://example.com/og.png" }], ogTitle: "A post" } },
        },
      ],
    });
    expect(neynarCast.hash).toBe("0xabc");
    expect(neynarCast.text).toBe("hi @dwr");
    expect(neynarCast.channel).toEqual({ id: "dev", label: "dev" });
    expect(neynarCast.embeds[0]).toEqual({
      kind: "link",
      url: "https://example.com/post",
      imageUrl: "https://example.com/og.png",
      title: "A post",
    });
  });
});

describe("NeynarShapedSource", () => {
  it("requests the trending feed and normalizes it", async () => {
    const { fetch, calls } = stubFetch({ "feed/trending": trending });
    const page = await hypersnapSource({ fetch }).trendingCasts({ limit: 250, cursor: "25" });
    expect(page.items).toHaveLength(trending.casts.length);
    expect(page.cursor).toBe("6");
    expect(calls[0].origin).toBe("https://haatz.quilibrium.com");
    expect(calls[0].searchParams.get("limit")).toBe("100"); // clamped to the provider maximum
    expect(calls[0].searchParams.get("cursor")).toBe("25");
  });

  it("looks up legacy channels by parent URL and keeps the friendly id", async () => {
    const { fetch, calls } = stubFetch({ channel: legacyChannel });
    const result = await hypersnapSource({ fetch }).channel("farcaster");
    expect(calls[0].searchParams.get("id")).toBe(FARCASTER_URL);
    expect(calls[0].searchParams.get("type")).toBe("parent_url");
    expect(result).toMatchObject({ id: "farcaster", name: "farcaster", parentUrl: FARCASTER_URL, memberCount: 94568 });
  });

  it("returns null for unknown channels, users and casts", async () => {
    const { fetch } = stubFetch({});
    const source = hypersnapSource({ fetch });
    await expect(source.channel("nope")).resolves.toBeNull();
    await expect(source.userByUsername("nobody")).resolves.toBeNull();
    await expect(source.conversation("0x" + "0".repeat(40))).resolves.toBeNull();
  });

  it("still serves feeds for parent URLs the provider has no channel record for", async () => {
    const { fetch } = stubFetch({});
    const result = await hypersnapSource({ fetch }).channel("https://example.com/article");
    expect(result).toMatchObject({ id: "https://example.com/article", name: "example.com/article", memberCount: null });
  });

  it("fetches channel feeds under every spelling of the parent URL", async () => {
    const { fetch, calls } = stubFetch({ "feed/parent_urls": { casts: [], next: {} } });
    const source = hypersnapSource({ fetch });
    const page = await source.channelCasts(FARCASTER_URL, { limit: 50 });
    expect(page).toEqual({ items: [], cursor: null });
    expect(calls[0].searchParams.get("parent_urls")).toBe(
      `${FARCASTER_URL},${FARCASTER_URL.replace("chain://", "")}`,
    );
    await source.channelCasts("https://warpcast.com/~/channel/gifart", { limit: 50 });
    expect(calls[1].searchParams.get("parent_urls")).toBe("https://warpcast.com/~/channel/gifart");
  });

  it("loads users and their casts", async () => {
    const { fetch } = stubFetch({ "user/by_username": user, "feed/user/casts": userCasts });
    const source = hypersnapSource({ fetch });
    const dwr = await source.userByUsername("dwr");
    expect(dwr).toMatchObject({ fid: 3, username: "dwr", displayName: "Dan Romero" });
    expect(dwr?.followerCount).toBeGreaterThan(1_000_000);
    const casts = await source.userCasts(3, { limit: 10 });
    expect(casts.items.filter((c) => c.parentHash)).toHaveLength(2);
  });

  it("finds ENS-style usernames that by_username can't resolve", async () => {
    // Live behaviour: by_username 404s for "balajis.eth"; search returns it
    // first, plus a decoy whose *display name* is "balajis.eth".
    const { fetch, calls } = stubFetch({ "user/search": userSearch });
    const balajis = await hypersnapSource({ fetch }).userByUsername("balajis.eth");
    expect(balajis).toMatchObject({ fid: 37, username: "balajis.eth", displayName: "balajis" });
    expect(balajis?.followerCount).toBeGreaterThan(500_000);
    expect(calls.map((url) => url.pathname.split("/farcaster/")[1])).toEqual(["user/by_username", "user/search"]);
    expect(calls[1].searchParams.get("q")).toBe("balajis.eth");
  });

  it("only accepts an exact username match from search", async () => {
    const { fetch } = stubFetch({ "user/search": userSearch });
    const source = hypersnapSource({ fetch });
    await expect(source.userByUsername("balajis")).resolves.toBeNull();
    // matched on username, never on display name
    await expect(source.userByUsername("balajis-eth")).resolves.toMatchObject({ fid: 1615452 });
    await expect(source.userByUsername("BALAJIS.ETH")).resolves.toMatchObject({ fid: 37 });
  });

  it("reads Neynar's nested search results", async () => {
    const { fetch } = stubFetch({ "user/search": { result: { users: userSearch.users } } });
    await expect(neynarSource({ apiKey: "k", fetch }).userByUsername("balajis.eth")).resolves.toMatchObject({
      fid: 37,
    });
  });

  it("doesn't search when by_username finds the user", async () => {
    const { fetch, calls } = stubFetch({ "user/by_username": user, "user/search": userSearch });
    await expect(hypersnapSource({ fetch }).userByUsername("dwr")).resolves.toMatchObject({ fid: 3 });
    expect(calls).toHaveLength(1);
  });

  it("sends the Neynar API key and throws on server errors", async () => {
    const { fetch, mock } = stubFetch({ "feed/trending": new Response("boom", { status: 502 }) });
    const source = neynarSource({ apiKey: "test-key", fetch });
    await expect(source.trendingCasts({ limit: 50 })).rejects.toBeInstanceOf(DataSourceError);
    const init = mock.mock.calls[0][1] as RequestInit;
    expect((init.headers as Record<string, string>)["x-api-key"]).toBe("test-key");
    expect(String(mock.mock.calls[0][0])).toContain("limit=10");
  });

  it("wraps network failures", async () => {
    const fetch = vi.fn(async () => {
      throw new TypeError("fetch failed");
    }) as unknown as typeof globalThis.fetch;
    await expect(hypersnapSource({ fetch }).trendingCasts({ limit: 5 })).rejects.toThrow(/request to \/feed\/trending failed/);
  });
});

describe("createDataSource", () => {
  it("defaults to Hypersnap", () => {
    expect(createDataSource({}).name).toBe("Hypersnap");
  });

  it("requires a key for Neynar", () => {
    expect(() => createDataSource({ ALLTHING_DATA_SOURCE: "neynar" })).toThrow(/NEYNAR_API_KEY/);
    expect(createDataSource({ ALLTHING_DATA_SOURCE: "neynar", NEYNAR_API_KEY: "k" }).name).toBe("Neynar");
  });

  it("rejects unknown sources", () => {
    expect(() => createDataSource({ ALLTHING_DATA_SOURCE: "myspace" })).toThrow(/Unknown/);
  });
});

describe("collectCasts + MemorySource", () => {
  const casts: Cast[] = trending.casts.map((raw) => normalizeCast(raw as RawCast));

  it("follows cursors until it has enough casts", async () => {
    const source = new MemorySource({ trending: casts });
    const fetchPage = vi.fn((cursor: string | null) => source.trendingCasts({ limit: 2, cursor }));
    const pool = await collectCasts(fetchPage, 5);
    expect(pool.map((c) => c.hash)).toEqual(casts.slice(0, 6).map((c) => c.hash));
    expect(fetchPage).toHaveBeenCalledTimes(3);
  });

  it("stops at the end of the feed and drops duplicates", async () => {
    const pages = [
      { items: [casts[0], casts[1]], cursor: "a" },
      { items: [casts[1], casts[2]], cursor: null },
    ];
    let i = 0;
    const pool = await collectCasts(async () => pages[i++], 100);
    expect(pool.map((c) => c.hash)).toEqual([casts[0].hash, casts[1].hash, casts[2].hash]);
  });
});

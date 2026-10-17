import { afterEach, describe, expect, it, vi } from "vitest";
import { MemorySource } from "@/lib/datasource/memory";
import { normalizeCast, normalizeThread, type RawCast } from "@/lib/datasource/normalize";
import { hashFromParam, usernameFromParam } from "@/lib/params";
import { loadChannelPage, loadFrontPage, loadPostPage, loadUserPage } from "@/lib/queries";
import type { Cast, Channel, User } from "@/lib/types";
import conversation from "./fixtures/hypersnap-conversation.json";
import trending from "./fixtures/hypersnap-feed-trending.json";

const casts: Cast[] = trending.casts.map((raw) => normalizeCast(raw as RawCast));
const thread = normalizeThread(conversation.conversation.cast as RawCast);
const gifart: Channel = {
  id: "gifart",
  name: "gifart",
  parentUrl: "https://warpcast.com/~/channel/gifart",
  description: null,
  imageUrl: null,
  memberCount: 2023,
  createdAt: null,
};
const dwr: User = {
  fid: 3,
  username: "dwr",
  displayName: "Dan Romero",
  pfpUrl: null,
  bio: "",
  followerCount: 1,
  followingCount: 1,
  registeredAt: null,
  verifiedAddresses: [],
};

describe("route helpers", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it("use paths in the server build", async () => {
    const links = await import("@/lib/links");
    expect(links.STATIC_EXPORT).toBe(false);
    expect(links.postHref("0xabc")).toBe("/p/0xabc");
    expect(links.channelHref("eip155:8453/erc20:0xabc")).toBe("/c/eip155%3A8453%2Ferc20%3A0xabc");
    expect(links.userHref("fid:3")).toBe("/u/fid%3A3");
    expect(links.hrefFor(links.channelRoute("dev"), { sort: "top", page: 2 })).toBe("/c/dev?sort=top&page=2");
  });

  it("use query strings in the static build", async () => {
    vi.stubEnv("NEXT_PUBLIC_ALLTHING_STATIC", "1");
    const links = await import("@/lib/links");
    expect(links.STATIC_EXPORT).toBe(true);
    expect(links.postHref("0xabc")).toBe("/p/?hash=0xabc");
    expect(links.channelHref("eip155:8453/erc20:0xabc")).toBe("/c/?id=eip155%3A8453%2Ferc20%3A0xabc");
    expect(links.userHref("dwr")).toBe("/u/?name=dwr");
    expect(links.hrefFor(links.channelRoute("dev"), { sort: "top", page: 2 })).toBe("/c/?id=dev&sort=top&page=2");
    expect(links.hrefFor(links.channelsRoute)).toBe("/channels/");
  });
});

describe("URL params", () => {
  it("normalizes cast hashes", () => {
    expect(hashFromParam("0xF454B8D3E5EE2C9252A0B7574AD4942E9BA502D2")).toBe(
      "0xf454b8d3e5ee2c9252a0b7574ad4942e9ba502d2",
    );
    expect(hashFromParam("f454b8d3e5ee2c9252a0b7574ad4942e9ba502d2")).toBe(
      "0xf454b8d3e5ee2c9252a0b7574ad4942e9ba502d2",
    );
    expect(hashFromParam("garbage")).toBeNull();
    expect(hashFromParam(null)).toBeNull();
  });

  it("normalizes usernames", () => {
    expect(usernameFromParam("@DWR")).toBe("dwr");
    expect(usernameFromParam("fid%3A3")).toBe("fid:3");
    expect(usernameFromParam("no spaces")).toBeNull();
    expect(usernameFromParam("%E0%A4%A")).toBeNull();
    expect(usernameFromParam("")).toBeNull();
    expect(usernameFromParam(null)).toBeNull();
  });

  it("accepts ENS-style names with dots and non-ASCII labels", () => {
    expect(usernameFromParam("balajis.eth")).toBe("balajis.eth");
    expect(usernameFromParam("Jesse.Base.eth")).toBe("jesse.base.eth");
    expect(usernameFromParam("base.base.eth")).toBe("base.base.eth");
    expect(usernameFromParam(encodeURIComponent("🌈.eth"))).toBe("🌈.eth");
    expect(usernameFromParam(".eth")).toBeNull();
    expect(usernameFromParam("a/b.eth")).toBeNull();
  });
});

describe("page loaders", () => {
  it("load the front page even when trending channels fail", async () => {
    const source = new MemorySource({ trending: casts });
    vi.spyOn(source, "trendingChannels").mockRejectedValue(new Error("down"));
    const data = await loadFrontPage(source);
    expect(data.pool).toHaveLength(casts.length);
    expect(data.trendingChannels).toEqual([]);
  });

  it("load a channel page newest first, or null for unknown channels", async () => {
    const shuffled = [casts[2], casts[0], casts[1]];
    const source = new MemorySource({ channels: [gifart], channelCasts: { [gifart.parentUrl]: shuffled } });
    const data = await loadChannelPage(source, "gifart");
    expect(data?.channel.id).toBe("gifart");
    const times = data!.pool.map((c) => Date.parse(c.timestamp));
    expect(times).toEqual([...times].sort((a, b) => b - a));
    await expect(loadChannelPage(source, "nope")).resolves.toBeNull();
  });

  it("load a post with its parent and quoted casts", async () => {
    const reply = { ...thread.children[0].cast, parentHash: casts[0].hash };
    const quoting = {
      ...casts[3],
      hash: "0x" + "1".repeat(40),
      embeds: [
        { kind: "cast" as const, hash: casts[1].hash, fid: 1 },
        { kind: "cast" as const, hash: "0x" + "2".repeat(40), fid: 2 },
      ],
    };
    const source = new MemorySource({
      trending: casts,
      threads: [
        { cast: reply, children: [] },
        { cast: quoting, children: [] },
      ],
    });
    const replyPage = await loadPostPage(source, reply.hash);
    expect(replyPage?.parent?.hash).toBe(casts[0].hash);

    const quotePage = await loadPostPage(source, quoting.hash);
    expect(quotePage?.quotes).toEqual([
      { hash: casts[1].hash, cast: casts[1] },
      { hash: "0x" + "2".repeat(40), cast: null },
    ]);
    await expect(loadPostPage(source, "0x" + "0".repeat(40))).resolves.toBeNull();
  });

  it("load users by name or by fid", async () => {
    const source = new MemorySource({ users: [dwr], userCasts: { 3: casts } });
    expect((await loadUserPage(source, "dwr"))?.pool).toHaveLength(casts.length);
    expect((await loadUserPage(source, "fid:3"))?.user.username).toBe("dwr");
    await expect(loadUserPage(source, "nobody")).resolves.toBeNull();
  });
});

describe("browser loader", () => {
  it("shares one request per key and forgets failures", async () => {
    const { load } = await import("@/client/load");
    const loader = vi.fn(async () => 42);
    const [a, b] = await Promise.all([load("answer", loader), load("answer", loader)]);
    expect(a.value).toBe(42);
    expect(b).toBe(a);
    expect(loader).toHaveBeenCalledTimes(1);

    const failing = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce("ok");
    await expect(load("flaky", failing)).rejects.toThrow("offline");
    await expect(load("flaky", failing)).resolves.toMatchObject({ value: "ok" });
  });
});

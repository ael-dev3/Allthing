import { describe, expect, it } from "vitest";
import {
  canonicalParentUrl,
  channelIdForUrl,
  channelKeyFromParam,
  channelRefFromUrl,
  channelUrlForId,
  parentUrlVariants,
  shortParentUrlLabel,
} from "@/lib/channels";
import { classifyUrl, embedDomain, resizedImageUrl, thumbnailUrl, visibleEmbeds } from "@/lib/embeds";
import { channelHref } from "@/lib/links";

const FARCASTER_URL = "chain://eip155:7777777/erc721:0x4f86113fc3e9783cf3ec9a552cbb566716a57628";

describe("channels", () => {
  it("maps standard channel URLs to ids", () => {
    expect(channelIdForUrl("https://warpcast.com/~/channel/gifart")).toBe("gifart");
    expect(channelIdForUrl("https://farcaster.xyz/~/channel/dev-tools")).toBe("dev-tools");
  });

  it("maps legacy parent URLs both ways", () => {
    expect(channelIdForUrl(FARCASTER_URL)).toBe("farcaster");
    expect(channelUrlForId("farcaster")).toBe(FARCASTER_URL);
    expect(channelUrlForId("gifart")).toBe("https://warpcast.com/~/channel/gifart");
  });

  it("labels non-channel parent URLs compactly", () => {
    expect(shortParentUrlLabel("eip155:8453/erc20:0x4ed4e862860bed51a9570b96d89af5e1b0efefed")).toBe(
      "erc20:0x4ed4…efed",
    );
    expect(shortParentUrlLabel("https://example.com/some/article/")).toBe("example.com/some/article");
    expect(channelRefFromUrl("eip155:8453/erc20:0x4ed4e862860bed51a9570b96d89af5e1b0efefed")).toEqual({
      id: "eip155:8453/erc20:0x4ed4e862860bed51a9570b96d89af5e1b0efefed",
      label: "erc20:0x4ed4…efed",
    });
    expect(channelRefFromUrl(null)).toBeNull();
  });

  it("treats chain:// and bare CAIP parent URLs as one feed", () => {
    const bare = "eip155:8453/erc20:0x4ed4e862860bed51a9570b96d89af5e1b0efefed";
    expect(canonicalParentUrl(`chain://${bare}`)).toBe(bare);
    expect(canonicalParentUrl(bare)).toBe(bare);
    expect(canonicalParentUrl("https://example.com/a")).toBe("https://example.com/a");
    expect(parentUrlVariants(bare)).toEqual([`chain://${bare}`, bare]);
    expect(parentUrlVariants("https://example.com/a")).toEqual(["https://example.com/a"]);
    expect(channelRefFromUrl(`chain://${bare}`)).toEqual(channelRefFromUrl(bare));
    expect(channelIdForUrl(FARCASTER_URL.replace("chain://", ""))).toBe("farcaster");
    expect(channelKeyFromParam(encodeURIComponent(`chain://${bare}`))).toBe(bare);
  });

  it("round-trips route params", () => {
    expect(channelHref("gifart")).toBe("/c/gifart");
    const href = channelHref("eip155:8453/erc20:0xabc");
    expect(href).toBe("/c/eip155%3A8453%2Ferc20%3A0xabc");
    expect(channelKeyFromParam(href.slice(3))).toBe("eip155:8453/erc20:0xabc");
    expect(channelKeyFromParam("GifArt")).toBe("gifart");
    expect(channelKeyFromParam("not a channel")).toBeNull();
    expect(channelKeyFromParam("%E0%A4%A")).toBeNull();
  });
});

describe("embeds", () => {
  it("classifies URLs", () => {
    expect(classifyUrl("https://imagedelivery.net/acct/id/original")).toBe("image");
    expect(classifyUrl("https://example.com/cat.JPG?w=1")).toBe("image");
    expect(classifyUrl("https://stream.warpcast.com/v1/video/abc.m3u8")).toBe("video");
    expect(classifyUrl("https://example.com/page")).toBe("link");
    expect(classifyUrl("https://example.com/page", "image/png")).toBe("image");
  });

  it("resizes Cloudflare Images URLs and leaves others alone", () => {
    expect(resizedImageUrl("https://imagedelivery.net/acct/abc-123/original", "thumb")).toBe(
      "https://imagedelivery.net/acct/abc-123/anim=false,fit=cover,f=auto,w=140,h=140",
    );
    expect(resizedImageUrl("https://i.imgur.com/x.png", "thumb")).toBe("https://i.imgur.com/x.png");
  });

  it("picks thumbnails from images and link previews only", () => {
    expect(thumbnailUrl({ kind: "image", url: "https://i.imgur.com/x.png" })).toBe("https://i.imgur.com/x.png");
    expect(
      thumbnailUrl({ kind: "link", url: "https://a.com", imageUrl: "https://a.com/og.png", title: null }),
    ).toBe("https://a.com/og.png");
    expect(thumbnailUrl({ kind: "link", url: "https://a.com", imageUrl: null, title: null })).toBeNull();
    expect(thumbnailUrl(undefined)).toBeNull();
  });

  it("skips link embeds the text already shows", () => {
    const link = { kind: "link" as const, url: "https://a.com/x", imageUrl: null, title: null };
    const image = { kind: "image" as const, url: "https://i.imgur.com/x.png" };
    expect(visibleEmbeds([link, image], "see https://a.com/x")).toEqual([image]);
    expect(visibleEmbeds([link], "no url here")).toEqual([link]);
  });

  it("extracts display domains", () => {
    expect(embedDomain("https://www.youtube.com/watch?v=1")).toBe("youtube.com");
    expect(embedDomain("not a url")).toBe("not a url");
  });
});

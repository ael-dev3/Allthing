import { describe, expect, it } from "vitest";
import { compactNumber, plural } from "@/lib/format";
import { firstParam, paginate, parsePage, queryString, repliesOnly, topLevelOnly } from "@/lib/listing";
import { channelRoute, sortTabs } from "@/lib/links";
import { countDescendants, countNodes, parseCommentSort, sortThread } from "@/lib/thread";
import { durationSince, formatDate, timeAgo } from "@/lib/time";
import type { Cast, ThreadNode } from "@/lib/types";

const NOW = Date.parse("2026-10-17T12:00:00.000Z");

function node(hash: string, likes: number, timestamp: string, children: ThreadNode[] = []): ThreadNode {
  const cast: Cast = {
    hash,
    author: { fid: 1, username: "a", displayName: "", pfpUrl: null },
    text: "",
    mentions: [],
    timestamp,
    parentHash: "0xparent",
    parentUrl: null,
    channel: null,
    embeds: [],
    likes,
    recasts: 0,
    replies: children.length,
  };
  return { cast, children };
}

describe("timeAgo", () => {
  it.each([
    ["2026-10-17T11:59:30.000Z", "just now"],
    ["2026-10-17T11:15:00.000Z", "45m ago"],
    ["2026-10-17T09:00:00.000Z", "3h ago"],
    ["2026-10-15T12:00:00.000Z", "2d ago"],
    ["2026-07-01T12:00:00.000Z", "3mo ago"],
    ["2024-01-01T12:00:00.000Z", "2y ago"],
    ["2026-10-17T12:05:00.000Z", "just now"], // clock skew: never "in the future"
  ])("%s -> %s", (iso, expected) => {
    expect(timeAgo(iso, NOW)).toBe(expected);
  });

  it("formats dates and durations", () => {
    expect(formatDate("2026-10-17T02:30:02.000Z")).toBe("17 Oct 2026");
    expect(durationSince("2023-10-17T12:00:00.000Z", NOW)).toBe("3 years");
    expect(durationSince("2026-10-16T12:00:00.000Z", NOW)).toBe("1 day");
    expect(timeAgo("garbage", NOW)).toBe("");
  });
});

describe("listing helpers", () => {
  it("paginates with continuing ranks", () => {
    const items = Array.from({ length: 60 }, (_, i) => i);
    const second = paginate(items, 2);
    expect(second.items[0]).toBe(25);
    expect(second.offset).toBe(25);
    expect(second.hasPrev).toBe(true);
    expect(second.hasNext).toBe(true);
    expect(paginate(items, 3).hasNext).toBe(false);
  });

  it("parses page numbers defensively", () => {
    expect(parsePage("3")).toBe(3);
    expect(parsePage("0")).toBe(1);
    expect(parsePage("abc")).toBe(1);
    expect(parsePage(undefined)).toBe(1);
    expect(parsePage("9999")).toBe(50);
  });

  it("separates posts from replies", () => {
    const post = node("0x1", 0, "2026-10-17T00:00:00.000Z").cast;
    const top = { ...post, parentHash: null };
    expect(topLevelOnly([top, post])).toEqual([top]);
    expect(repliesOnly([top, post])).toEqual([post]);
  });

  it("builds query strings and tabs", () => {
    expect(queryString({ sort: "new", page: 2, empty: "" })).toBe("?sort=new&page=2");
    expect(queryString({ sort: undefined })).toBe("");
    expect(firstParam(["a", "b"])).toBe("a");
    expect(sortTabs(channelRoute("gifart"), "new")).toEqual([
      { label: "hot", href: "/c/gifart", selected: false },
      { label: "new", href: "/c/gifart?sort=new", selected: true },
      { label: "top", href: "/c/gifart?sort=top", selected: false },
    ]);
  });

  it("formats numbers like old reddit", () => {
    expect(compactNumber(999)).toBe("999");
    expect(compactNumber(12_345)).toBe("12.3k");
    expect(compactNumber(250_000)).toBe("250k");
    expect(compactNumber(1_221_547)).toBe("1.2m");
    expect(plural(1, "comment")).toBe("1 comment");
    expect(plural(1200, "comment")).toBe("1,200 comments");
    expect(plural(2, "child", "children")).toBe("2 children");
  });
});

describe("thread helpers", () => {
  const tree = [
    node("0xa", 1, "2026-10-17T01:00:00.000Z", [
      node("0xa1", 0, "2026-10-17T03:00:00.000Z"),
      node("0xa2", 9, "2026-10-17T02:00:00.000Z", [node("0xa2x", 0, "2026-10-17T04:00:00.000Z")]),
    ]),
    node("0xb", 5, "2026-10-17T00:30:00.000Z"),
  ];

  it("counts nested replies", () => {
    expect(countDescendants(tree[0])).toBe(3);
    expect(countNodes(tree)).toBe(5);
  });

  it("sorts every level", () => {
    const top = sortThread(tree, "top");
    expect(top.map((n) => n.cast.hash)).toEqual(["0xb", "0xa"]);
    expect(top[1].children.map((n) => n.cast.hash)).toEqual(["0xa2", "0xa1"]);
    const old = sortThread(tree, "old");
    expect(old.map((n) => n.cast.hash)).toEqual(["0xb", "0xa"]);
    expect(old[1].children.map((n) => n.cast.hash)).toEqual(["0xa2", "0xa1"]);
    const fresh = sortThread(tree, "new");
    expect(fresh[0].children.map((n) => n.cast.hash)).toEqual(["0xa1", "0xa2"]);
  });

  it("parses comment sorts", () => {
    expect(parseCommentSort("old")).toBe("old");
    expect(parseCommentSort("hot")).toBe("top");
  });
});

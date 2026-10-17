import { describe, expect, it } from "vitest";
import { castScore, hotScore, parseSort, sortCasts } from "@/lib/ranking";
import type { Cast } from "@/lib/types";

function cast(hash: string, likes: number, recasts: number, timestamp: string): Cast {
  return {
    hash,
    author: { fid: 1, username: "a", displayName: "", pfpUrl: null },
    text: hash,
    mentions: [],
    timestamp,
    parentHash: null,
    parentUrl: null,
    channel: null,
    embeds: [],
    likes,
    recasts,
    replies: 0,
  };
}

describe("castScore", () => {
  it("counts recasts double", () => {
    expect(castScore({ likes: 10, recasts: 3 })).toBe(16);
  });
});

describe("hotScore", () => {
  // Reference values from reddit's _sorts.pyx:
  //   order = log10(max(abs(s), 1)); sign = 1|0|-1
  //   round(sign * order + (epoch_seconds - 1134028003) / 45000, 7)
  it("matches reddit's formula", () => {
    const at = "2026-10-17T00:00:00.000Z";
    const seconds = Date.parse(at) / 1000 - 1134028003;
    expect(hotScore(100, at)).toBeCloseTo(2 + seconds / 45000, 6);
    expect(hotScore(1, at)).toBeCloseTo(seconds / 45000, 6);
    expect(hotScore(0, at)).toBeCloseTo(seconds / 45000, 6);
  });

  it("trades 10x the score for 12.5 hours", () => {
    const later = hotScore(10, "2026-10-17T12:30:00.000Z");
    const earlier = hotScore(100, "2026-10-17T00:00:00.000Z");
    expect(later).toBeCloseTo(earlier, 6);
  });
});

describe("sortCasts", () => {
  const old = cast("0xold", 500, 50, "2026-10-14T00:00:00.000Z"); // score 600, 3 days old
  const fresh = cast("0xfresh", 20, 2, "2026-10-17T10:00:00.000Z"); // score 24, today
  const mid = cast("0xmid", 100, 0, "2026-10-16T12:00:00.000Z"); // score 100, yesterday

  it("new: newest first", () => {
    expect(sortCasts([old, fresh, mid], "new").map((c) => c.hash)).toEqual(["0xfresh", "0xmid", "0xold"]);
  });

  it("top: highest likes + 2 x recasts first", () => {
    expect(sortCasts([fresh, mid, old], "top").map((c) => c.hash)).toEqual(["0xold", "0xmid", "0xfresh"]);
  });

  it("hot: recency beats a big but stale score", () => {
    expect(sortCasts([old, mid, fresh], "hot").map((c) => c.hash)).toEqual(["0xfresh", "0xmid", "0xold"]);
  });

  it("does not mutate its input", () => {
    const input = [old, fresh];
    sortCasts(input, "new");
    expect(input.map((c) => c.hash)).toEqual(["0xold", "0xfresh"]);
  });
});

describe("parseSort", () => {
  it("accepts known sorts and falls back otherwise", () => {
    expect(parseSort("top")).toBe("top");
    expect(parseSort("controversial")).toBe("hot");
    expect(parseSort(undefined, "new")).toBe("new");
  });
});

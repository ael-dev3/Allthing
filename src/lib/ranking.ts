import type { Cast } from "./types";

export const SORTS = ["hot", "new", "top"] as const;
export type SortKey = (typeof SORTS)[number];

export function parseSort(value: unknown, fallback: SortKey = "hot"): SortKey {
  return typeof value === "string" && (SORTS as readonly string[]).includes(value)
    ? (value as SortKey)
    : fallback;
}

/** A cast's "points": likes count once, recasts count double. */
export function castScore(cast: Pick<Cast, "likes" | "recasts">): number {
  return cast.likes + 2 * cast.recasts;
}

/** Seconds since the Unix epoch at which reddit's hot ranking starts counting. */
const REDDIT_EPOCH_SECONDS = 1134028003;

/**
 * Reddit's "hot" ranking (r2/lib/db/_sorts.pyx): log10 of the score plus a
 * time bonus, so 10x the points buys 12.5 hours of freshness.
 */
export function hotScore(score: number, timestamp: string | number | Date): number {
  const order = Math.log10(Math.max(Math.abs(score), 1));
  const sign = score > 0 ? 1 : score < 0 ? -1 : 0;
  const seconds = new Date(timestamp).getTime() / 1000 - REDDIT_EPOCH_SECONDS;
  return Math.round((sign * order + seconds / 45000) * 1e7) / 1e7;
}

function time(cast: Cast): number {
  return new Date(cast.timestamp).getTime();
}

function byNewest(a: Cast, b: Cast): number {
  return time(b) - time(a) || a.hash.localeCompare(b.hash);
}

export function sortCasts(casts: readonly Cast[], sort: SortKey): Cast[] {
  const sorted = [...casts];
  switch (sort) {
    case "new":
      return sorted.sort(byNewest);
    case "top":
      return sorted.sort((a, b) => castScore(b) - castScore(a) || byNewest(a, b));
    case "hot":
      return sorted.sort(
        (a, b) =>
          hotScore(castScore(b), b.timestamp) - hotScore(castScore(a), a.timestamp) ||
          byNewest(a, b),
      );
  }
}

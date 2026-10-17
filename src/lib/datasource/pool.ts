import type { Cast, Page } from "../types";

/**
 * Follow cursors until `want` unique casts are collected, the feed ends, or
 * `maxPages` requests have been made. Hot/top sorting needs a pool of casts
 * to rank, and providers cap page sizes differently.
 */
export async function collectCasts(
  fetchPage: (cursor: string | null) => Promise<Page<Cast>>,
  want: number,
  maxPages = 5,
): Promise<Cast[]> {
  const seen = new Set<string>();
  const casts: Cast[] = [];
  let cursor: string | null = null;

  for (let page = 0; page < maxPages && casts.length < want; page++) {
    const result = await fetchPage(cursor);
    for (const cast of result.items) {
      if (!seen.has(cast.hash)) {
        seen.add(cast.hash);
        casts.push(cast);
      }
    }
    if (!result.cursor || result.cursor === cursor || result.items.length === 0) break;
    cursor = result.cursor;
  }
  return casts;
}

import type { Cast } from "./types";

export const PAGE_SIZE = 25;
const MAX_PAGE = 50;

export interface PageSlice<T> {
  items: T[];
  page: number;
  /** Number of items before this page, so ranks continue across pages. */
  offset: number;
  hasPrev: boolean;
  hasNext: boolean;
}

export function parsePage(value: unknown): number {
  const page = typeof value === "string" ? Number.parseInt(value, 10) : Number.NaN;
  return Number.isFinite(page) && page >= 1 ? Math.min(page, MAX_PAGE) : 1;
}

export function paginate<T>(items: readonly T[], page: number, size = PAGE_SIZE): PageSlice<T> {
  const offset = (page - 1) * size;
  return {
    items: items.slice(offset, offset + size),
    page,
    offset,
    hasPrev: page > 1,
    hasNext: offset + size < items.length,
  };
}

/** Listings show posts, not replies. */
export function topLevelOnly(casts: readonly Cast[]): Cast[] {
  return casts.filter((cast) => !cast.parentHash);
}

export function repliesOnly(casts: readonly Cast[]): Cast[] {
  return casts.filter((cast) => Boolean(cast.parentHash));
}

/** First value of a search param that may be repeated. */
export function firstParam(value: string | string[] | undefined): string | undefined {
  return Array.isArray(value) ? value[0] : value;
}

/** Build "?sort=new&page=2", leaving out defaults. */
export function queryString(params: Record<string, string | number | undefined | null>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== "") search.set(key, String(value));
  }
  const query = search.toString();
  return query ? `?${query}` : "";
}

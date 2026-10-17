import { SORTS, type SortKey } from "./ranking";
import { queryString } from "./listing";

export function postHref(hash: string): string {
  return `/p/${hash}`;
}

export function userHref(username: string): string {
  return `/u/${encodeURIComponent(username)}`;
}

/** farcaster.xyz permalink for a cast: /<username>/<first 10 chars of the hash>. */
export function farcasterCastUrl(username: string, hash: string): string {
  return `https://farcaster.xyz/${encodeURIComponent(username)}/${hash.slice(0, 10)}`;
}

export interface Tab {
  label: string;
  href: string;
  selected: boolean;
}

/** hot/new/top tabs for a listing path; "hot" is the default and stays out of the URL. */
export function sortTabs(path: string, current: SortKey, extra: Record<string, string | undefined> = {}): Tab[] {
  return SORTS.map((sort) => ({
    label: sort,
    href: path + queryString({ ...extra, sort: sort === "hot" ? undefined : sort }),
    selected: sort === current,
  }));
}

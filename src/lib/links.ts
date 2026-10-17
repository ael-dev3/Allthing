import { queryString } from "./listing";
import { SORTS, type SortKey } from "./ranking";

/**
 * Two URL schemes. The server build uses paths (/c/farcaster, /p/0x…). The
 * static GitHub Pages build can only serve pages that exist at build time, so
 * it puts the variable part in the query string (/c/?id=farcaster). Chosen at
 * build time; see next.config.ts.
 */
export const STATIC_EXPORT = process.env.NEXT_PUBLIC_ALLTHING_STATIC === "1";

export type Query = Record<string, string | number | undefined | null>;

export interface Route {
  pathname: string;
  query?: Query;
}

export function hrefFor(route: Route, extra: Query = {}): string {
  return route.pathname + queryString({ ...route.query, ...extra });
}

export const homeRoute: Route = { pathname: "/" };
export const channelsRoute: Route = { pathname: STATIC_EXPORT ? "/channels/" : "/channels" };

export function postRoute(hash: string): Route {
  return STATIC_EXPORT ? { pathname: "/p/", query: { hash } } : { pathname: `/p/${hash}` };
}

/** A channel id ("farcaster") or the parent URL of a non-channel feed. */
export function channelRoute(channel: string | { id: string }): Route {
  const id = typeof channel === "string" ? channel : channel.id;
  return STATIC_EXPORT ? { pathname: "/c/", query: { id } } : { pathname: `/c/${encodeURIComponent(id)}` };
}

export function userRoute(username: string): Route {
  return STATIC_EXPORT
    ? { pathname: "/u/", query: { name: username } }
    : { pathname: `/u/${encodeURIComponent(username)}` };
}

export function postHref(hash: string): string {
  return hrefFor(postRoute(hash));
}

export function channelHref(channel: string | { id: string }): string {
  return hrefFor(channelRoute(channel));
}

export function userHref(username: string): string {
  return hrefFor(userRoute(username));
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

/** hot/new/top tabs for a listing; the default sort stays out of the URL. */
export function sortTabs(route: Route, current: SortKey, defaultSort: SortKey = "hot"): Tab[] {
  return SORTS.map((sort) => ({
    label: sort,
    href: hrefFor(route, { sort: sort === defaultSort ? undefined : sort }),
    selected: sort === current,
  }));
}

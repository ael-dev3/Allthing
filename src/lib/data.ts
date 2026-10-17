import { cacheLife } from "next/cache";
import { connection } from "next/server";
import { isChannelId } from "./channels";
import { getDataSource } from "./datasource";
import { collectCasts } from "./datasource/pool";
import type { Cast, Channel, ThreadNode, User } from "./types";

/*
 * Cached reads for pages. Each function is a `use cache` boundary keyed by
 * its arguments, so a channel's feed is fetched at most once a minute no
 * matter how many sort orders and pages are rendered from it.
 */

/** How many casts listings rank over. Hot and top are computed within this pool. */
export const POOL_SIZE = 100;

function newestFirst(a: Cast, b: Cast): number {
  return new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime();
}

export async function getTrendingPool(): Promise<Cast[]> {
  "use cache";
  cacheLife("minutes");
  const source = getDataSource();
  const casts = await collectCasts((cursor) => source.trendingCasts({ limit: POOL_SIZE, cursor }), POOL_SIZE);
  return casts.slice(0, POOL_SIZE);
}

export async function getChannel(key: string): Promise<Channel | null> {
  "use cache";
  cacheLife("hours");
  return getDataSource().channel(key);
}

export async function getChannelPool(parentUrl: string): Promise<Cast[]> {
  "use cache";
  cacheLife("minutes");
  const source = getDataSource();
  const casts = await collectCasts(
    (cursor) => source.channelCasts(parentUrl, { limit: POOL_SIZE, cursor }),
    POOL_SIZE,
  );
  // Hypersnap merges several shards per page, so order isn't guaranteed.
  return casts.sort(newestFirst).slice(0, POOL_SIZE);
}

/** Named channels only; token and URL feeds crowd the trending list otherwise. */
export async function getTrendingChannels(limit = 15): Promise<Channel[]> {
  "use cache";
  cacheLife("hours");
  const channels = await getDataSource().trendingChannels(100);
  return channels.filter((channel) => isChannelId(channel.id)).slice(0, limit);
}

export async function searchChannels(query: string): Promise<Channel[]> {
  "use cache";
  cacheLife("hours");
  return getDataSource().searchChannels(query, 50);
}

/** Look up a user by fname, or by fid with the "fid:<n>" form used for fname-less accounts. */
export async function getUser(username: string): Promise<User | null> {
  "use cache";
  cacheLife("hours");
  const source = getDataSource();
  const fid = /^fid:(\d+)$/.exec(username);
  return fid ? source.userByFid(Number(fid[1])) : source.userByUsername(username);
}

export async function getUserPool(fid: number): Promise<Cast[]> {
  "use cache";
  cacheLife("minutes");
  const source = getDataSource();
  const casts = await collectCasts((cursor) => source.userCasts(fid, { limit: POOL_SIZE, cursor }), POOL_SIZE);
  return casts.sort(newestFirst).slice(0, POOL_SIZE);
}

export async function getCast(hash: string): Promise<Cast | null> {
  "use cache";
  cacheLife("minutes");
  return getDataSource().cast(hash);
}

export async function getThread(hash: string): Promise<ThreadNode | null> {
  "use cache";
  cacheLife("minutes");
  return getDataSource().conversation(hash);
}

/**
 * Current time for "3h ago" labels. Waits for the request so the value is
 * never frozen into a prerendered shell.
 */
export async function requestNow(): Promise<number> {
  await connection();
  return Date.now();
}

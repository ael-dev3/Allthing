import { cacheLife } from "next/cache";
import { connection } from "next/server";
import { getDataSource } from "./datasource";
import {
  loadChannelDirectory,
  loadChannelPage,
  loadFrontPage,
  loadPostPage,
  loadUserPage,
  type ChannelPageData,
  type FrontPageData,
  type PostPageData,
  type UserPageData,
} from "./queries";
import type { Channel } from "./types";

/*
 * Cached page data for the server build. Each function is a `use cache`
 * boundary keyed by its arguments, so a channel's feed is fetched at most
 * once a minute no matter how many sort orders and pages are rendered from it.
 */

export async function getFrontPage(): Promise<FrontPageData> {
  "use cache";
  cacheLife("minutes");
  return loadFrontPage(getDataSource());
}

export async function getChannelPage(key: string): Promise<ChannelPageData | null> {
  "use cache";
  cacheLife("minutes");
  return loadChannelPage(getDataSource(), key);
}

export async function getPostPage(hash: string): Promise<PostPageData | null> {
  "use cache";
  cacheLife("minutes");
  return loadPostPage(getDataSource(), hash);
}

export async function getUserPage(username: string): Promise<UserPageData | null> {
  "use cache";
  cacheLife("minutes");
  return loadUserPage(getDataSource(), username);
}

export async function getChannelDirectory(query: string): Promise<Channel[]> {
  "use cache";
  cacheLife("hours");
  return loadChannelDirectory(getDataSource(), query);
}

/**
 * Current time for "3h ago" labels. Waits for the request so the value is
 * never frozen into a prerendered shell.
 */
export async function requestNow(): Promise<number> {
  await connection();
  return Date.now();
}

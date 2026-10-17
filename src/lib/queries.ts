import { channelUrlForId, isChannelId } from "./channels";
import type { DataSource } from "./datasource/types";
import { collectCasts } from "./datasource/pool";
import type { Cast, Channel, ThreadNode, User } from "./types";

/*
 * Everything a page needs, loaded through a DataSource. Framework-agnostic:
 * the server build wraps these in `use cache` (src/lib/data.ts), the static
 * build calls them from the browser (src/client/load.ts).
 */

/** How many casts listings rank over. Hot and top are computed within this pool. */
export const POOL_SIZE = 100;

function newestFirst(a: Cast, b: Cast): number {
  return new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime();
}

export async function trendingPool(source: DataSource): Promise<Cast[]> {
  const casts = await collectCasts((cursor) => source.trendingCasts({ limit: POOL_SIZE, cursor }), POOL_SIZE);
  return casts.slice(0, POOL_SIZE);
}

export async function channelPool(source: DataSource, parentUrl: string): Promise<Cast[]> {
  const casts = await collectCasts(
    (cursor) => source.channelCasts(parentUrl, { limit: POOL_SIZE, cursor }),
    POOL_SIZE,
  );
  // Hypersnap merges several shards per page, so order isn't guaranteed.
  return casts.sort(newestFirst).slice(0, POOL_SIZE);
}

export async function userPool(source: DataSource, fid: number): Promise<Cast[]> {
  const casts = await collectCasts((cursor) => source.userCasts(fid, { limit: POOL_SIZE, cursor }), POOL_SIZE);
  return casts.sort(newestFirst).slice(0, POOL_SIZE);
}

/** Named channels only; token and URL feeds crowd the trending list otherwise. */
export async function trendingChannels(source: DataSource, limit = 15): Promise<Channel[]> {
  const channels = await source.trendingChannels(100);
  return channels.filter((channel) => isChannelId(channel.id)).slice(0, limit);
}

/** Look up a user by fname, or by fid with the "fid:<n>" form used for fname-less accounts. */
export async function findUser(source: DataSource, username: string): Promise<User | null> {
  const fid = /^fid:(\d+)$/.exec(username);
  return fid ? source.userByFid(Number(fid[1])) : source.userByUsername(username);
}

export interface FrontPageData {
  pool: Cast[];
  trendingChannels: Channel[];
}

export async function loadFrontPage(source: DataSource): Promise<FrontPageData> {
  const [pool, channels] = await Promise.all([
    trendingPool(source),
    trendingChannels(source).catch(() => []), // the sidebar is optional
  ]);
  return { pool, trendingChannels: channels };
}

export interface ChannelPageData {
  channel: Channel;
  pool: Cast[];
}

export async function loadChannelPage(source: DataSource, key: string): Promise<ChannelPageData | null> {
  // The feed URL is almost always predictable from the key, so fetch it
  // alongside the channel lookup instead of after it.
  const guess = isChannelId(key) ? channelUrlForId(key) : key;
  const [channel, guessedPool] = await Promise.all([
    source.channel(key),
    channelPool(source, guess).catch(() => null),
  ]);
  if (!channel) return null;
  const pool =
    guessedPool && channel.parentUrl === guess ? guessedPool : await channelPool(source, channel.parentUrl);
  return { channel, pool };
}

export interface QuotedCastData {
  hash: string;
  cast: Cast | null;
}

export interface PostPageData {
  thread: ThreadNode;
  /** The channel the cast was posted in, for the sidebar. */
  channel: Channel | null;
  /** The cast this one replies to, when it is a reply. */
  parent: Cast | null;
  quotes: QuotedCastData[];
}

export async function loadPostPage(source: DataSource, hash: string): Promise<PostPageData | null> {
  const thread = await source.conversation(hash);
  if (!thread) return null;
  const root = thread.cast;
  const quoteHashes = root.embeds.flatMap((embed) => (embed.kind === "cast" ? [embed.hash] : []));
  // Context around the cast is best-effort; the thread itself is what matters.
  const [channel, parent, quotes] = await Promise.all([
    root.channel ? source.channel(root.channel.id).catch(() => null) : null,
    root.parentHash ? source.cast(root.parentHash).catch(() => null) : null,
    Promise.all(
      quoteHashes.map(async (quoted) => ({ hash: quoted, cast: await source.cast(quoted).catch(() => null) })),
    ),
  ]);
  return { thread, channel, parent, quotes };
}

export interface UserPageData {
  user: User;
  pool: Cast[];
}

export async function loadUserPage(source: DataSource, username: string): Promise<UserPageData | null> {
  const user = await findUser(source, username);
  if (!user) return null;
  return { user, pool: await userPool(source, user.fid) };
}

export async function loadChannelDirectory(source: DataSource, query: string): Promise<Channel[]> {
  const channels = query ? await source.searchChannels(query, 50) : await trendingChannels(source, 50);
  return channels.slice(0, 50);
}

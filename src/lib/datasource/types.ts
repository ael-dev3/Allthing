import type { Cast, Channel, Page, ThreadNode, User } from "../types";

export interface PageRequest {
  limit: number;
  cursor?: string | null;
}

/**
 * Everything Allthing reads. Implementations must be read-only, return the
 * normalized types from ../types, and resolve to `null` (not throw) when the
 * requested entity doesn't exist. Transport failures should throw.
 *
 * Today: NeynarShapedSource (Hypersnap or Neynar). A Snapchain-node source
 * would implement the same interface on top of the hub HTTP API.
 */
export interface DataSource {
  /** Shown in the footer, e.g. "Hypersnap". */
  readonly name: string;

  trendingCasts(request: PageRequest): Promise<Page<Cast>>;
  trendingChannels(limit: number): Promise<Channel[]>;
  searchChannels(query: string, limit: number): Promise<Channel[]>;

  /** Look up a channel by id ("farcaster") or by parent URL. */
  channel(key: string): Promise<Channel | null>;
  /** Casts posted under a parent URL, newest first where the provider allows. */
  channelCasts(parentUrl: string, request: PageRequest): Promise<Page<Cast>>;

  userByUsername(username: string): Promise<User | null>;
  userByFid(fid: number): Promise<User | null>;
  /** A user's casts, top-level posts and replies mixed, newest first. */
  userCasts(fid: number, request: PageRequest): Promise<Page<Cast>>;

  /** A single cast. `hash` is 0x-prefixed. */
  cast(hash: string): Promise<Cast | null>;
  /** A cast and its reply tree. `hash` is 0x-prefixed. */
  conversation(hash: string): Promise<ThreadNode | null>;
}

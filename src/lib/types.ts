/**
 * Normalized domain types. Every DataSource returns these plain, serializable
 * objects, so pages never see a provider's raw JSON shape.
 */

export interface UserSummary {
  fid: number;
  /** fname, or "fid:<n>" when the account has none. */
  username: string;
  displayName: string;
  pfpUrl: string | null;
}

export interface User extends UserSummary {
  bio: string;
  followerCount: number;
  followingCount: number;
  registeredAt: string | null;
  verifiedAddresses: string[];
}

export type Embed =
  | { kind: "image"; url: string }
  | { kind: "video"; url: string }
  | { kind: "link"; url: string; imageUrl: string | null; title: string | null }
  | { kind: "cast"; hash: string; fid: number | null };

export interface Mention {
  username: string;
  fid: number;
}

export interface ChannelRef {
  /** Route key for /c/[channel]: a channel id ("farcaster") or a raw parent URL. */
  id: string;
  /** Short display label ("farcaster", "erc20:0x4ed4…efed"). */
  label: string;
}

export interface Cast {
  /** 0x-prefixed, lowercase. */
  hash: string;
  author: UserSummary;
  /** Cast text with mentions spliced in as "@username". */
  text: string;
  mentions: Mention[];
  timestamp: string;
  parentHash: string | null;
  parentUrl: string | null;
  channel: ChannelRef | null;
  embeds: Embed[];
  likes: number;
  recasts: number;
  replies: number;
}

export interface Channel {
  /** Route key, same meaning as ChannelRef.id. */
  id: string;
  name: string;
  parentUrl: string;
  description: string | null;
  imageUrl: string | null;
  memberCount: number | null;
  createdAt: string | null;
}

export interface ThreadNode {
  cast: Cast;
  children: ThreadNode[];
}

export interface Page<T> {
  items: T[];
  cursor: string | null;
}

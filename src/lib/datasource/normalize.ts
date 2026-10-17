import {
  canonicalParentUrl,
  channelIdForUrl,
  channelRefFromUrl,
  channelUrlForId,
  shortParentUrlLabel,
} from "../channels";
import { classifyUrl } from "../embeds";
import { spliceMentions } from "../text";
import type { Cast, Channel, Embed, Mention, ThreadNode, User, UserSummary } from "../types";

/*
 * Raw shapes of the Neynar v2 API. Hypersnap serves the same shape with
 * fewer fields, so everything past the identifiers is optional.
 */

export interface RawUser {
  fid: number;
  username?: string | null;
  display_name?: string | null;
  pfp_url?: string | null;
  profile?: { bio?: { text?: string | null } | null } | null;
  follower_count?: number | null;
  following_count?: number | null;
  registered_at?: string | null;
  verified_addresses?: { eth_addresses?: string[]; sol_addresses?: string[] } | null;
}

export interface RawEmbed {
  url?: string;
  cast_id?: { fid?: number; hash?: string };
  cast?: { hash?: string; author?: { fid?: number } };
  metadata?: {
    content_type?: string | null;
    html?: { ogImage?: { url?: string }[] | null; ogTitle?: string | null } | null;
  } | null;
}

export interface RawCast {
  hash: string;
  text?: string | null;
  timestamp?: string | null;
  author?: RawUser | null;
  parent_hash?: string | null;
  parent_url?: string | null;
  root_parent_url?: string | null;
  channel?: { id?: string | null } | null;
  embeds?: RawEmbed[] | null;
  reactions?: { likes_count?: number | null; recasts_count?: number | null } | null;
  replies?: { count?: number | null } | null;
  mentioned_profiles?: RawUser[] | null;
  mentioned_profiles_ranges?: { start: number; end: number }[] | null;
  direct_replies?: RawCast[] | null;
}

export interface RawChannel {
  id: string;
  name?: string | null;
  url?: string | null;
  parent_url?: string | null;
  description?: string | null;
  image_url?: string | null;
  member_count?: number | null;
  follower_count?: number | null;
  created_at?: string | number | null;
}

/** Hashes are compared and routed as 0x-prefixed lowercase hex. */
export function normalizeHash(hash: string): string {
  const lower = hash.trim().toLowerCase();
  return lower.startsWith("0x") ? lower : `0x${lower}`;
}

function usernameFor(raw: RawUser): string {
  return raw.username?.trim() || `fid:${raw.fid}`;
}

export function normalizeUserSummary(raw: RawUser): UserSummary {
  return {
    fid: raw.fid,
    username: usernameFor(raw),
    displayName: raw.display_name?.trim() || "",
    pfpUrl: raw.pfp_url || null,
  };
}

export function normalizeUser(raw: RawUser): User {
  return {
    ...normalizeUserSummary(raw),
    bio: raw.profile?.bio?.text ?? "",
    followerCount: raw.follower_count ?? 0,
    followingCount: raw.following_count ?? 0,
    registeredAt: raw.registered_at || null,
    verifiedAddresses: [
      ...(raw.verified_addresses?.eth_addresses ?? []),
      ...(raw.verified_addresses?.sol_addresses ?? []),
    ],
  };
}

export function normalizeEmbed(raw: RawEmbed): Embed | null {
  const quoted = raw.cast_id ?? (raw.cast ? { hash: raw.cast.hash, fid: raw.cast.author?.fid } : null);
  if (quoted?.hash) {
    return { kind: "cast", hash: normalizeHash(quoted.hash), fid: quoted.fid ?? null };
  }
  if (!raw.url) return null;
  const kind = classifyUrl(raw.url, raw.metadata?.content_type);
  if (kind !== "link") return { kind, url: raw.url };
  return {
    kind,
    url: raw.url,
    imageUrl: raw.metadata?.html?.ogImage?.[0]?.url || null,
    title: raw.metadata?.html?.ogTitle || null,
  };
}

export function normalizeCast(raw: RawCast): Cast {
  const mentions: Mention[] = (raw.mentioned_profiles ?? []).map((user) => ({
    username: usernameFor(user),
    fid: user.fid,
  }));
  const channelUrl = raw.root_parent_url || raw.parent_url || null;
  const channel = raw.channel?.id
    ? { id: raw.channel.id, label: raw.channel.id }
    : channelRefFromUrl(channelUrl);

  return {
    hash: normalizeHash(raw.hash),
    author: normalizeUserSummary(raw.author ?? { fid: 0 }),
    text: spliceMentions(raw.text ?? "", mentions, raw.mentioned_profiles_ranges ?? []),
    mentions,
    timestamp: raw.timestamp ?? new Date(0).toISOString(),
    parentHash: raw.parent_hash ? normalizeHash(raw.parent_hash) : null,
    parentUrl: raw.parent_url || null,
    channel,
    embeds: (raw.embeds ?? []).map(normalizeEmbed).filter((e): e is Embed => e !== null),
    likes: raw.reactions?.likes_count ?? 0,
    recasts: raw.reactions?.recasts_count ?? 0,
    replies: raw.replies?.count ?? 0,
  };
}

export function normalizeThread(raw: RawCast): ThreadNode {
  return {
    cast: normalizeCast(raw),
    children: (raw.direct_replies ?? []).map(normalizeThread),
  };
}

function isoDate(value: string | number | null | undefined): string | null {
  if (value === null || value === undefined || value === "") return null;
  // Neynar sends unix seconds; Hypersnap sends ISO strings.
  const date = typeof value === "number" ? new Date(value * 1000) : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

export function normalizeChannel(raw: RawChannel): Channel {
  // Hypersnap uses the parent URL itself as the id of URL-keyed feeds.
  const idIsUrl = raw.id.includes(":") || raw.id.includes("/");
  const parentUrl = raw.parent_url || raw.url || (idIsUrl ? raw.id : channelUrlForId(raw.id));
  const namedId = idIsUrl ? channelIdForUrl(parentUrl) : raw.id.toLowerCase();
  const id = namedId ?? canonicalParentUrl(parentUrl);
  // Neynar's `name` is a display name ("Farcaster"); Hypersnap echoes the id.
  const displayName = !idIsUrl && raw.name?.trim() ? raw.name.trim() : null;
  return {
    id,
    name: displayName ?? namedId ?? shortParentUrlLabel(parentUrl),
    parentUrl,
    description: raw.description?.trim() || null,
    imageUrl: raw.image_url || null,
    memberCount: raw.member_count ?? raw.follower_count ?? null,
    createdAt: isoDate(raw.created_at),
  };
}

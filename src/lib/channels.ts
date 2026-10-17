import type { ChannelRef } from "./types";

/**
 * Channels created before Farcaster standardised on
 * `https://warpcast.com/~/channel/<id>` parent URLs. Their casts carry these
 * legacy URLs, so an id lookup that assumes the standard URL finds nothing.
 * Each entry was checked against live feeds.
 */
export const LEGACY_CHANNEL_URLS: Readonly<Record<string, string>> = {
  farcaster: "chain://eip155:7777777/erc721:0x4f86113fc3e9783cf3ec9a552cbb566716a57628",
  base: "https://onchainsummer.xyz",
  ethereum: "https://ethereum.org",
  memes: "chain://eip155:1/erc721:0xfd8427165df67df6d7fd689ae67c8ebf56d9ca61",
  nouns: "chain://eip155:1/erc721:0x9c8ff314c9bc7f6e59a9d9225fb22946427edc03",
  purple: "chain://eip155:1/erc721:0xa45662638e9f3bbb7a6fecb4b17853b7ba0f3a60",
  degen: "chain://eip155:7777777/erc721:0x5d6a07d07354f8793d1ca06280c4adf04767ad7e",
  dev: "chain://eip155:1/erc721:0x7dd4e31f1530ac682c8ea4d8016e95773e08d8b0",
  founders: "https://farcaster.group/founders",
  food: "chain://eip155:1/erc721:0xec0ba367a6edf483a252c3b093f012b9b1da8b3f",
};

/** The default strip of channels across the top, like old reddit's default subreddits. */
export const DEFAULT_CHANNELS = [
  "farcaster",
  "base",
  "ethereum",
  "degen",
  "memes",
  "dev",
  "founders",
  "food",
  "nouns",
  "itookaphoto",
  "diet",
  "veg",
  "gifart",
  "japan",
] as const;

const LEGACY_IDS_BY_URL = new Map(
  Object.entries(LEGACY_CHANNEL_URLS).map(([id, url]) => [url.toLowerCase(), id]),
);

const CHANNEL_URL_RE =
  /^https?:\/\/(?:www\.)?(?:warpcast\.com|farcaster\.xyz)\/~\/channel\/([a-z0-9][a-z0-9-]*)\/?$/i;
const CHANNEL_ID_RE = /^[a-z0-9][a-z0-9-]{0,63}$/;

export function isChannelId(value: string): boolean {
  return CHANNEL_ID_RE.test(value);
}

export function channelUrlForId(id: string): string {
  return LEGACY_CHANNEL_URLS[id] ?? `https://warpcast.com/~/channel/${id}`;
}

// CAIP-2 chain id ("eip155:8453", "solana:101") followed by an asset path.
const CAIP_URL_RE = /^[-a-z0-9]{3,8}:[-_a-zA-Z0-9]{1,32}\//;
const CHAIN_SCHEME = "chain://";

/**
 * Token and NFT feeds are written both as "chain://eip155:8453/erc20:0x…" and
 * as bare "eip155:8453/erc20:0x…". Route keys use the bare form.
 */
export function canonicalParentUrl(url: string): string {
  const bare = url.startsWith(CHAIN_SCHEME) ? url.slice(CHAIN_SCHEME.length) : url;
  return CAIP_URL_RE.test(bare) ? bare : url;
}

/** Every spelling of a parent URL that casts may have been posted under. */
export function parentUrlVariants(url: string): string[] {
  const bare = canonicalParentUrl(url);
  return CAIP_URL_RE.test(bare) ? [`${CHAIN_SCHEME}${bare}`, bare] : [url];
}

export function channelIdForUrl(url: string): string | null {
  const match = CHANNEL_URL_RE.exec(url);
  if (match) return match[1].toLowerCase();
  for (const variant of parentUrlVariants(url)) {
    const id = LEGACY_IDS_BY_URL.get(variant.toLowerCase());
    if (id) return id;
  }
  return null;
}

function truncate(value: string, max: number): string {
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

/** Short label for a parent URL that isn't a named channel (token feeds, web pages). */
export function shortParentUrlLabel(url: string): string {
  // chain://eip155:1/erc721:0xabc…, eip155:8453/erc20:0xabc…, solana:101/address:Abc…
  const asset = /(?:^|\/)([a-z0-9]+):(0x[0-9a-f]{40}|[1-9A-HJ-NP-Za-km-z]{32,44})$/i.exec(url);
  if (asset) {
    const [, kind, address] = asset;
    return `${kind}:${address.slice(0, 6)}…${address.slice(-4)}`;
  }
  try {
    const parsed = new URL(url);
    if (parsed.protocol === "http:" || parsed.protocol === "https:") {
      return truncate((parsed.host + parsed.pathname).replace(/\/$/, ""), 32);
    }
  } catch {
    // fall through to the raw string
  }
  return truncate(url, 32);
}

export function channelRefFromUrl(url: string | null | undefined): ChannelRef | null {
  if (!url) return null;
  const id = channelIdForUrl(url);
  if (id) return { id, label: id };
  return { id: canonicalParentUrl(url), label: shortParentUrlLabel(url) };
}

/** Display label for a channel key: the id itself, or a shortened parent URL. */
export function channelLabel(key: string): string {
  return isChannelId(key) ? key : (channelIdForUrl(key) ?? shortParentUrlLabel(key));
}

export function channelHref(ref: { id: string } | string): string {
  return `/c/${encodeURIComponent(typeof ref === "string" ? ref : ref.id)}`;
}

/**
 * Turn the raw /c/[channel] route segment back into a channel key: a
 * lowercase channel id, or the parent URL for non-channel feeds.
 */
export function channelKeyFromParam(param: string): string | null {
  let value = param;
  if (value.includes("%")) {
    try {
      value = decodeURIComponent(value);
    } catch {
      return null;
    }
  }
  value = value.trim();
  if (!value) return null;
  if (isChannelId(value.toLowerCase())) return value.toLowerCase();
  // Anything else must look like a parent URL (scheme:rest).
  return /^[a-z][a-z0-9+.-]*:/i.test(value) ? canonicalParentUrl(value) : null;
}

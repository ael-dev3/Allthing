import {
  LEGACY_CHANNEL_URLS,
  canonicalParentUrl,
  channelIdForUrl,
  parentUrlVariants,
  shortParentUrlLabel,
} from "../channels";
import type { Cast, Channel, Page, ThreadNode, User } from "../types";
import {
  normalizeCast,
  normalizeChannel,
  normalizeHash,
  normalizeThread,
  normalizeUser,
  type RawCast,
  type RawChannel,
  type RawUser,
} from "./normalize";
import type { DataSource, PageRequest } from "./types";

export const HYPERSNAP_BASE_URL = "https://haatz.quilibrium.com/v2/farcaster";
export const NEYNAR_BASE_URL = "https://api.neynar.com/v2/farcaster";

type Endpoint = "trending" | "feed" | "userCasts" | "channels";

export interface NeynarShapedConfig {
  name: string;
  baseUrl: string;
  /** Sent as `x-api-key`. Hypersnap needs none. */
  apiKey?: string;
  /** Largest `limit` each endpoint accepts. */
  maxLimits: Record<Endpoint, number>;
  timeoutMs?: number;
  /** Injectable for tests. */
  fetch?: typeof fetch;
}

export class DataSourceError extends Error {
  constructor(
    message: string,
    readonly status: number | null,
  ) {
    super(message);
    this.name = "DataSourceError";
  }
}

type Params = Record<string, string | number | boolean | null | undefined>;

interface FeedResponse {
  casts?: RawCast[];
  next?: { cursor?: string | null } | null;
}

interface ChannelListResponse {
  // Neynar wraps trending entries as { object: "channel_activity", channel }.
  channels?: (RawChannel | { channel: RawChannel })[];
}

function unwrapChannel(entry: RawChannel | { channel: RawChannel }): RawChannel {
  return "channel" in entry && typeof entry.channel === "object" ? entry.channel : (entry as RawChannel);
}

function toPage(response: FeedResponse | null): Page<Cast> {
  return {
    items: (response?.casts ?? []).map(normalizeCast),
    cursor: response?.next?.cursor || null,
  };
}

/**
 * Reads any API that speaks Neynar's v2 Farcaster shape: the free Hypersnap
 * endpoint, Neynar itself, or a self-hosted Hypersnap/Snapchain bridge.
 */
export class NeynarShapedSource implements DataSource {
  readonly name: string;
  private readonly config: NeynarShapedConfig;

  constructor(config: NeynarShapedConfig) {
    this.name = config.name;
    this.config = { ...config, baseUrl: config.baseUrl.replace(/\/+$/, "") };
  }

  private limit(endpoint: Endpoint, requested: number): number {
    return Math.max(1, Math.min(requested, this.config.maxLimits[endpoint]));
  }

  /** GET a JSON endpoint; resolves `null` on 404 and on 400 for malformed ids. */
  private async get<T>(path: string, params: Params, { nullOn400 = false } = {}): Promise<T | null> {
    const url = new URL(`${this.config.baseUrl}/${path}`);
    for (const [key, value] of Object.entries(params)) {
      if (value !== undefined && value !== null && value !== "") url.searchParams.set(key, String(value));
    }
    const headers: Record<string, string> = { accept: "application/json" };
    // Browsers forbid (or preflight) a custom User-Agent; only servers send one.
    if (typeof window === "undefined") headers["user-agent"] = "allthing/0.1 (+https://github.com/ael-dev3/allthing)";
    if (this.config.apiKey) headers["x-api-key"] = this.config.apiKey;

    const doFetch = this.config.fetch ?? fetch;
    let response: Response;
    try {
      response = await doFetch(url, {
        headers,
        signal: AbortSignal.timeout(this.config.timeoutMs ?? 15_000),
      });
    } catch (error) {
      throw new DataSourceError(`${this.name}: request to /${path} failed (${String(error)})`, null);
    }
    if (response.status === 404 || (nullOn400 && response.status === 400)) return null;
    if (!response.ok) {
      throw new DataSourceError(`${this.name}: /${path} returned HTTP ${response.status}`, response.status);
    }
    return (await response.json()) as T;
  }

  async trendingCasts({ limit, cursor }: PageRequest): Promise<Page<Cast>> {
    return toPage(
      await this.get<FeedResponse>("feed/trending", { limit: this.limit("trending", limit), cursor }),
    );
  }

  async trendingChannels(limit: number): Promise<Channel[]> {
    const response = await this.get<ChannelListResponse>("channel/trending", {
      limit: this.limit("channels", limit),
    });
    return (response?.channels ?? []).map((entry) => normalizeChannel(unwrapChannel(entry)));
  }

  async searchChannels(query: string, limit: number): Promise<Channel[]> {
    const response = await this.get<ChannelListResponse>("channel/search", {
      q: query,
      limit: this.limit("channels", limit),
    });
    return (response?.channels ?? []).map((entry) => normalizeChannel(unwrapChannel(entry)));
  }

  async channel(key: string): Promise<Channel | null> {
    const legacyUrl = LEGACY_CHANNEL_URLS[key];
    const isUrl = !legacyUrl && (key.includes(":") || key.includes("/"));

    if (!legacyUrl && !isUrl) {
      const response = await this.get<{ channel?: RawChannel }>("channel", { id: key }, { nullOn400: true });
      return response?.channel ? normalizeChannel(response.channel) : null;
    }

    // Legacy channels and raw parent URLs are looked up by URL. Their casts
    // live under that URL, so fall back to a bare channel if the provider
    // has no record of it.
    const parentUrl = legacyUrl ?? key;
    const response = await this.get<{ channel?: RawChannel }>(
      "channel",
      { id: parentUrl, type: "parent_url" },
      { nullOn400: true },
    );
    const channel = response?.channel ? normalizeChannel(response.channel) : null;
    return {
      id: channel?.id ?? channelIdForUrl(parentUrl) ?? canonicalParentUrl(parentUrl),
      name: channel?.name ?? (legacyUrl ? key : shortParentUrlLabel(parentUrl)),
      parentUrl,
      description: channel?.description ?? null,
      imageUrl: channel?.imageUrl ?? null,
      memberCount: channel?.memberCount ?? null,
      createdAt: channel?.createdAt ?? null,
    };
  }

  async channelCasts(parentUrl: string, { limit, cursor }: PageRequest): Promise<Page<Cast>> {
    return toPage(
      await this.get<FeedResponse>("feed/parent_urls", {
        parent_urls: parentUrlVariants(parentUrl).join(","),
        limit: this.limit("feed", limit),
        cursor,
        with_recasts: false,
      }),
    );
  }

  async userByUsername(username: string): Promise<User | null> {
    const response = await this.get<{ user?: RawUser }>("user/by_username", { username });
    return response?.user ? normalizeUser(response.user) : null;
  }

  async userByFid(fid: number): Promise<User | null> {
    const response = await this.get<{ users?: RawUser[] }>("user/bulk", { fids: fid });
    const user = response?.users?.[0];
    return user ? normalizeUser(user) : null;
  }

  async userCasts(fid: number, { limit, cursor }: PageRequest): Promise<Page<Cast>> {
    return toPage(
      await this.get<FeedResponse>("feed/user/casts", {
        fid,
        limit: this.limit("userCasts", limit),
        cursor,
        include_replies: true,
      }),
    );
  }

  async cast(hash: string): Promise<Cast | null> {
    const response = await this.get<{ cast?: RawCast }>(
      "cast",
      { identifier: normalizeHash(hash), type: "hash" },
      { nullOn400: true },
    );
    return response?.cast ? normalizeCast(response.cast) : null;
  }

  async conversation(hash: string): Promise<ThreadNode | null> {
    const response = await this.get<{ conversation?: { cast?: RawCast } }>(
      "cast/conversation",
      { identifier: normalizeHash(hash), type: "hash", reply_depth: 5, limit: 50 },
      { nullOn400: true },
    );
    const root = response?.conversation?.cast;
    return root ? normalizeThread(root) : null;
  }
}

export function hypersnapSource(options: { baseUrl?: string; fetch?: typeof fetch } = {}): NeynarShapedSource {
  return new NeynarShapedSource({
    name: "Hypersnap",
    baseUrl: options.baseUrl ?? HYPERSNAP_BASE_URL,
    maxLimits: { trending: 100, feed: 100, userCasts: 100, channels: 100 },
    fetch: options.fetch,
  });
}

export function neynarSource(options: { apiKey: string; baseUrl?: string; fetch?: typeof fetch }): NeynarShapedSource {
  return new NeynarShapedSource({
    name: "Neynar",
    baseUrl: options.baseUrl ?? NEYNAR_BASE_URL,
    apiKey: options.apiKey,
    maxLimits: { trending: 10, feed: 100, userCasts: 150, channels: 25 },
    fetch: options.fetch,
  });
}

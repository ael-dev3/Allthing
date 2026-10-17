import type { Cast, Channel, Page, ThreadNode, User } from "../types";
import type { DataSource, PageRequest } from "./types";

export interface MemoryData {
  trending?: Cast[];
  channels?: Channel[];
  /** Casts keyed by channel parent URL. */
  channelCasts?: Record<string, Cast[]>;
  users?: User[];
  /** Casts keyed by fid. */
  userCasts?: Record<number, Cast[]>;
  threads?: ThreadNode[];
}

function page<T>(items: T[], { limit, cursor }: PageRequest): Page<T> {
  const start = cursor ? Number(cursor) : 0;
  const end = start + limit;
  return { items: items.slice(start, end), cursor: end < items.length ? String(end) : null };
}

/** In-memory DataSource for tests and offline development. */
export class MemorySource implements DataSource {
  readonly name = "memory";

  constructor(private readonly data: MemoryData) {}

  async trendingCasts(request: PageRequest) {
    return page(this.data.trending ?? [], request);
  }

  async trendingChannels(limit: number) {
    return (this.data.channels ?? []).slice(0, limit);
  }

  async searchChannels(query: string, limit: number) {
    const q = query.toLowerCase();
    return (this.data.channels ?? []).filter((c) => c.id.includes(q) || c.name.includes(q)).slice(0, limit);
  }

  async channel(key: string) {
    return (this.data.channels ?? []).find((c) => c.id === key || c.parentUrl === key) ?? null;
  }

  async channelCasts(parentUrl: string, request: PageRequest) {
    return page(this.data.channelCasts?.[parentUrl] ?? [], request);
  }

  async userByUsername(username: string) {
    return (this.data.users ?? []).find((u) => u.username === username) ?? null;
  }

  async userByFid(fid: number) {
    return (this.data.users ?? []).find((u) => u.fid === fid) ?? null;
  }

  async userCasts(fid: number, request: PageRequest) {
    return page(this.data.userCasts?.[fid] ?? [], request);
  }

  async cast(hash: string) {
    const all = [
      ...(this.data.trending ?? []),
      ...Object.values(this.data.channelCasts ?? {}).flat(),
      ...Object.values(this.data.userCasts ?? {}).flat(),
    ];
    return all.find((c) => c.hash === hash) ?? null;
  }

  async conversation(hash: string) {
    return (this.data.threads ?? []).find((t) => t.cast.hash === hash) ?? null;
  }
}

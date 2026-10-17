import { hypersnapSource, neynarSource } from "./neynar";
import type { DataSource } from "./types";

export type { DataSource, PageRequest } from "./types";

type Env = Record<string, string | undefined>;

/**
 * Pick the data source from the environment:
 *
 *   ALLTHING_DATA_SOURCE   hypersnap (default) | neynar
 *   ALLTHING_API_BASE_URL  override the base URL, e.g. a self-hosted Hypersnap
 *   NEYNAR_API_KEY         required for neynar
 */
export function createDataSource(env: Env = process.env): DataSource {
  const kind = (env.ALLTHING_DATA_SOURCE || "hypersnap").trim().toLowerCase();
  const baseUrl = env.ALLTHING_API_BASE_URL?.trim() || undefined;

  switch (kind) {
    case "hypersnap":
      return hypersnapSource({ baseUrl });
    case "neynar": {
      const apiKey = env.NEYNAR_API_KEY?.trim();
      if (!apiKey) throw new Error("NEYNAR_API_KEY is required when ALLTHING_DATA_SOURCE=neynar");
      return neynarSource({ apiKey, baseUrl });
    }
    default:
      throw new Error(`Unknown ALLTHING_DATA_SOURCE "${kind}" (expected "hypersnap" or "neynar")`);
  }
}

let current: DataSource | undefined;

export function getDataSource(): DataSource {
  current ??= createDataSource();
  return current;
}

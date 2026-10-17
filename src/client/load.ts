import { createDataSource, type DataSource } from "@/lib/datasource";

/*
 * Browser-side data loading for the static (GitHub Pages) build. The
 * Hypersnap API sends `Access-Control-Allow-Origin: *`, so pages fetch it
 * directly. Results are memoized per key for a minute, which keeps sort and
 * page changes instant and gives React's `use()` a stable promise.
 */

const TTL_MS = 60_000;

export interface Loaded<T> {
  value: T;
  /** When the data arrived; used as "now" for relative times. */
  loadedAt: number;
}

const entries = new Map<string, { expires: number; promise: Promise<Loaded<unknown>> }>();

export function load<T>(key: string, loader: () => Promise<T>): Promise<Loaded<T>> {
  const now = Date.now();
  const hit = entries.get(key);
  if (hit && hit.expires > now) return hit.promise as Promise<Loaded<T>>;

  const promise = loader().then((value) => ({ value, loadedAt: Date.now() }));
  entries.set(key, { expires: now + TTL_MS, promise });
  // Don't cache failures: a retry should hit the network again.
  promise.catch(() => {
    if (entries.get(key)?.promise === promise) entries.delete(key);
  });
  return promise;
}

let source: DataSource | undefined;

/** Always Hypersnap in the browser: it is keyless and CORS-enabled. */
export function browserSource(): DataSource {
  source ??= createDataSource({
    ALLTHING_DATA_SOURCE: "hypersnap",
    ALLTHING_API_BASE_URL: process.env.NEXT_PUBLIC_ALLTHING_API_BASE_URL,
  });
  return source;
}

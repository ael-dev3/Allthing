import { normalizeHash } from "./datasource/normalize";

const HASH_RE = /^(?:0x)?[0-9a-f]{40}$/i;
const USERNAME_RE = /^(?:fid:\d+|[a-z0-9][a-z0-9._-]{0,63})$/;

function decode(value: string): string | null {
  if (!value.includes("%")) return value;
  try {
    return decodeURIComponent(value);
  } catch {
    return null;
  }
}

/** A cast hash from a URL, 0x-prefixed and lowercase, or null if malformed. */
export function hashFromParam(param: string | null | undefined): string | null {
  const value = param?.trim() ?? "";
  return HASH_RE.test(value) ? normalizeHash(value) : null;
}

/** A username ("dwr", "@DWR", "fid:3") from a URL, normalized, or null if malformed. */
export function usernameFromParam(param: string | null | undefined): string | null {
  const value = decode(param ?? "")
    ?.trim()
    .replace(/^@/, "")
    .toLowerCase();
  return value && USERNAME_RE.test(value) ? value : null;
}

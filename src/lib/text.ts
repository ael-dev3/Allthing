import { embedDomain } from "./embeds";
import type { Cast, Mention } from "./types";

export interface ByteRange {
  start: number;
  end: number;
}

/**
 * Farcaster stores mentions out of band: the text omits them and
 * `mentions_positions` give UTF-8 byte offsets where "@username" belongs.
 * Hypersnap passes those through as zero-width ranges; Neynar returns text
 * with mentions already spliced in (non-zero-width ranges). Handle both.
 */
export function spliceMentions(text: string, mentions: Mention[], ranges: ByteRange[]): string {
  if (mentions.length === 0 || ranges.length !== mentions.length) return text;
  if (!ranges.every((range) => range.start === range.end)) return text;

  const bytes = new TextEncoder().encode(text);
  const decoder = new TextDecoder();
  const inserts = ranges
    .map((range, i) => ({ at: range.start, mention: mentions[i] }))
    .sort((a, b) => a.at - b.at);

  let out = "";
  let cursor = 0;
  for (const { at, mention } of inserts) {
    const position = Math.min(Math.max(at, cursor), bytes.length);
    out += decoder.decode(bytes.subarray(cursor, position)) + `@${mention.username}`;
    cursor = position;
  }
  return out + decoder.decode(bytes.subarray(cursor));
}

export const TITLE_MAX_CHARS = 120;

/** First ~120 characters of a cast, whitespace-collapsed, cut at a word boundary. */
export function truncateTitle(text: string, max = TITLE_MAX_CHARS): string {
  const flat = text.replace(/\s+/g, " ").trim();
  const chars = Array.from(flat); // code points, so emoji aren't split
  if (chars.length <= max) return flat;
  const cut = chars.slice(0, max).join("");
  const space = cut.lastIndexOf(" ");
  const base = space > max * 0.6 ? cut.slice(0, space) : cut;
  return `${base.replace(/[\s.,;:!?…-]+$/u, "")}…`;
}

/** The listing title for a cast, with a placeholder for text-less casts. */
export function castTitle(cast: Pick<Cast, "text" | "embeds">): string {
  const title = truncateTitle(cast.text);
  if (title) return title;
  const embed = cast.embeds[0];
  switch (embed?.kind) {
    case "image":
      return "[image]";
    case "video":
      return "[video]";
    case "link":
      return `[link] ${embed.title ?? embedDomain(embed.url)}`;
    case "cast":
      return "[quote cast]";
    default:
      return "[empty cast]";
  }
}

/** Whether the post page should repeat the full text below the title. */
export function needsBody(text: string): boolean {
  const trimmed = text.trim();
  if (!trimmed) return false;
  return trimmed.includes("\n") || truncateTitle(trimmed) !== trimmed.replace(/\s+/g, " ");
}

export type TextToken =
  | { type: "text"; text: string }
  | { type: "url"; text: string; href: string }
  | { type: "mention"; text: string; username: string }
  | { type: "channel"; text: string; channel: string };

const TOKEN_RE =
  /(https?:\/\/[^\s<>"']+)|(@[a-z0-9][a-z0-9_.-]*[a-z0-9]|@[a-z0-9])|((?<=^|[\s(])\/[a-z0-9][a-z0-9-]{0,31}(?![\w/-]))/gi;
const TRAILING_PUNCT_RE = /[.,!?;:'"]+$|\)+$/;

function trimUrl(raw: string): string {
  let url = raw;
  for (;;) {
    const match = TRAILING_PUNCT_RE.exec(url);
    if (!match) return url;
    // keep a closing paren if the URL itself opened one, e.g. wikipedia links
    if (match[0].startsWith(")")) {
      const open = (url.match(/\(/g) ?? []).length;
      const close = (url.match(/\)/g) ?? []).length;
      if (open >= close) return url;
      url = url.slice(0, -1);
      continue;
    }
    url = url.slice(0, match.index);
  }
}

/**
 * Split cast text into plain text, URLs, mentions and /channel references.
 * Only "@names" that the cast actually mentions become links.
 */
export function tokenize(text: string, mentions: Mention[] = []): TextToken[] {
  const known = new Set(mentions.map((m) => m.username.toLowerCase()));
  const tokens: TextToken[] = [];
  let last = 0;

  const pushText = (value: string) => {
    if (!value) return;
    const prev = tokens[tokens.length - 1];
    if (prev?.type === "text") prev.text += value;
    else tokens.push({ type: "text", text: value });
  };

  for (const match of text.matchAll(TOKEN_RE)) {
    const start = match.index;
    const [whole, url, mention, channel] = match;
    pushText(text.slice(last, start));
    last = start + whole.length;

    if (url) {
      const href = trimUrl(url);
      tokens.push({ type: "url", text: href, href });
      pushText(url.slice(href.length));
    } else if (mention) {
      const username = mention.slice(1);
      if (known.has(username.toLowerCase())) {
        tokens.push({ type: "mention", text: mention, username });
      } else {
        pushText(mention);
      }
    } else if (channel) {
      tokens.push({ type: "channel", text: channel, channel: channel.slice(1).toLowerCase() });
    } else {
      pushText(whole);
    }
  }
  pushText(text.slice(last));
  return tokens;
}

/** Split text into paragraphs on blank lines; single newlines are kept inside. */
export function paragraphs(text: string): string[] {
  return text
    .split(/\n\s*\n/)
    .map((p) => p.replace(/^\n+|\n+$/g, ""))
    .filter((p) => p.trim().length > 0);
}

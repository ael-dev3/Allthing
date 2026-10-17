import type { Embed } from "./types";

const IMAGE_EXT_RE = /\.(?:png|jpe?g|gif|webp|avif|bmp|svg)(?:$|[?#])/i;
const VIDEO_EXT_RE = /\.(?:mp4|webm|mov|m3u8)(?:$|[?#])/i;
const IMAGE_HOSTS = new Set(["imagedelivery.net", "i.imgur.com", "media.tenor.com"]);
const VIDEO_HOSTS = new Set(["stream.warpcast.com", "stream.farcaster.xyz"]);

function hostOf(url: string): string | null {
  try {
    return new URL(url).hostname.toLowerCase();
  } catch {
    return null;
  }
}

/** Decide how to show an embedded URL, using a content type when the provider has one. */
export function classifyUrl(url: string, contentType?: string | null): "image" | "video" | "link" {
  if (contentType?.startsWith("image/")) return "image";
  if (contentType?.startsWith("video/") || contentType === "application/x-mpegurl") return "video";
  const host = hostOf(url);
  if (VIDEO_EXT_RE.test(url) || (host && VIDEO_HOSTS.has(host))) return "video";
  if (IMAGE_EXT_RE.test(url) || (host && IMAGE_HOSTS.has(host))) return "image";
  return "link";
}

/** "i.imgur.com" for a URL; the raw string if it can't be parsed. */
export function embedDomain(url: string): string {
  const host = hostOf(url);
  return host ? host.replace(/^www\./, "") : url;
}

const IMAGEDELIVERY_RE = /^(https:\/\/imagedelivery\.net\/[^/]+\/[^/]+)\/[^/?#]+$/;

/**
 * Cloudflare Images (used by most Farcaster clients) serves resized variants
 * on the fly, which turns a ~800 KB original into a ~6 KB thumbnail.
 */
export function resizedImageUrl(url: string, variant: "thumb" | "full"): string {
  const match = IMAGEDELIVERY_RE.exec(url);
  if (!match) return url;
  return variant === "thumb"
    ? `${match[1]}/anim=false,fit=cover,f=auto,w=140,h=140`
    : `${match[1]}/fit=scale-down,f=auto,w=1200`;
}

/** Image to show in the 70px listing thumbnail slot, if the embed has one. */
export function thumbnailUrl(embed: Embed | undefined): string | null {
  if (!embed) return null;
  if (embed.kind === "image") return resizedImageUrl(embed.url, "thumb");
  if (embed.kind === "link" && embed.imageUrl) return resizedImageUrl(embed.imageUrl, "thumb");
  return null;
}

/**
 * Embeds worth rendering under a cast's text: images and quotes always, links
 * and videos only when the text doesn't already show the URL.
 */
export function visibleEmbeds(embeds: Embed[], text: string): Embed[] {
  return embeds.filter((embed) => embed.kind === "image" || embed.kind === "cast" || !text.includes(embed.url));
}

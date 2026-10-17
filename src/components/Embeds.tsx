import Link from "next/link";
import { embedDomain, resizedImageUrl, visibleEmbeds } from "@/lib/embeds";
import { postHref } from "@/lib/links";
import type { Embed } from "@/lib/types";

/**
 * Embedded media under a cast's text: images inline, everything else as
 * links. Pass the cast text to skip links the text already shows.
 */
export function Embeds({ embeds, text = "", compact = false }: { embeds: Embed[]; text?: string; compact?: boolean }) {
  const shown = visibleEmbeds(embeds, text);
  if (shown.length === 0) return null;
  return (
    <div className={compact ? "embeds compact" : "embeds"}>
      {shown.map((embed, i) => {
        switch (embed.kind) {
          case "image":
            return (
              <a key={i} className="embed-image" href={embed.url}>
                {/* eslint-disable-next-line @next/next/no-img-element -- remote, arbitrary hosts */}
                <img
                  src={resizedImageUrl(embed.url, "full")}
                  alt="embedded image"
                  loading="lazy"
                  decoding="async"
                  referrerPolicy="no-referrer"
                />
              </a>
            );
          case "video":
            return (
              <p key={i} className="embed-link">
                <span className="embed-kind">video</span>{" "}
                <a href={embed.url} rel="nofollow ugc noopener noreferrer">
                  {embed.url}
                </a>
              </p>
            );
          case "link":
            return (
              <p key={i} className="embed-link">
                <span className="embed-kind">link</span>{" "}
                <a href={embed.url} rel="nofollow ugc noopener noreferrer">
                  {embed.title ?? embed.url}
                </a>{" "}
                <span className="domain">({embedDomain(embed.url)})</span>
              </p>
            );
          case "cast":
            return (
              <p key={i} className="embed-link">
                <span className="embed-kind">quote</span>{" "}
                <Link href={postHref(embed.hash)}>quoted cast {embed.hash.slice(0, 10)}</Link>
              </p>
            );
        }
      })}
    </div>
  );
}

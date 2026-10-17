import Link from "next/link";
import { channelHref } from "@/lib/channels";
import { embedDomain, thumbnailUrl } from "@/lib/embeds";
import { compactNumber, plural } from "@/lib/format";
import { postHref, userHref } from "@/lib/links";
import { castScore } from "@/lib/ranking";
import { castTitle } from "@/lib/text";
import type { Cast } from "@/lib/types";
import { TimeAgo } from "./TimeAgo";

const ICONS = {
  self: (
    <path d="M9 9h22v3H9zm0 7h22v3H9zm0 7h14v3H9z" fill="currentColor" />
  ),
  link: (
    <path
      d="M17.5 21.5l-2 2a4 4 0 01-5.6-5.6l4-4a4 4 0 015.6 0l-1.6 1.6a1.8 1.8 0 00-2.4 0l-4 4a1.8 1.8 0 002.4 2.4l1.2-1.2a5.8 5.8 0 002.4.8zm5-11l2-2a4 4 0 015.6 5.6l-4 4a4 4 0 01-5.6 0l1.6-1.6a1.8 1.8 0 002.4 0l4-4a1.8 1.8 0 00-2.4-2.4l-1.2 1.2a5.8 5.8 0 00-2.4-.8z"
      fill="currentColor"
    />
  ),
  video: <path d="M15 9l12 8-12 8z" fill="currentColor" />,
  quote: (
    <path
      d="M9 24v-6c0-4 2-7 6-8l1 2c-2 1-3 2-3 4h3v8zm11 0v-6c0-4 2-7 6-8l1 2c-2 1-3 2-3 4h3v8z"
      fill="currentColor"
    />
  ),
};

function PlaceholderThumb({ kind, href, external }: { kind: keyof typeof ICONS; href: string; external?: boolean }) {
  const icon = (
    <svg viewBox="0 0 40 34" width="40" height="34" aria-hidden="true">
      {ICONS[kind]}
    </svg>
  );
  return external ? (
    <a className={`thumbnail placeholder ${kind}`} href={href} rel="nofollow noopener noreferrer" aria-label={kind}>
      {icon}
    </a>
  ) : (
    <Link className={`thumbnail placeholder ${kind}`} href={href} aria-label={kind} tabIndex={-1}>
      {icon}
    </Link>
  );
}

function Thumbnail({ cast }: { cast: Cast }) {
  const embed = cast.embeds[0];
  const image = thumbnailUrl(embed);
  if (image && embed && embed.kind !== "cast") {
    return (
      <a className="thumbnail" href={embed.url} rel="nofollow noopener noreferrer" tabIndex={-1}>
        {/* eslint-disable-next-line @next/next/no-img-element -- remote, arbitrary hosts */}
        <img src={image} alt="" width={70} height={70} loading="lazy" decoding="async" referrerPolicy="no-referrer" />
      </a>
    );
  }
  switch (embed?.kind) {
    case "video":
      return <PlaceholderThumb kind="video" href={embed.url} external />;
    case "link":
      return <PlaceholderThumb kind="link" href={embed.url} external />;
    case "cast":
      return <PlaceholderThumb kind="quote" href={postHref(embed.hash)} />;
    default:
      return <PlaceholderThumb kind="self" href={postHref(cast.hash)} />;
  }
}

/** "(i.imgur.com)" for media posts, "(self.channel)" for text posts, like old reddit. */
function domainLabel(cast: Cast): string {
  const embed = cast.embeds[0];
  if (!embed) return `self.${cast.channel?.label ?? "farcaster"}`;
  if (embed.kind === "cast") return "quote";
  return embedDomain(embed.url);
}

/** One line of a listing: rank, score, thumbnail, title, and the "submitted …" tagline. */
export function PostRow({ cast, rank, now }: { cast: Cast; rank?: number; now: number }) {
  const href = postHref(cast.hash);
  const score = castScore(cast);
  return (
    <div className="thing link" data-nav="" data-hash={cast.hash}>
      {rank !== undefined && <span className="rank">{rank}</span>}
      <div className="midcol">
        <span className="arrow up" aria-hidden="true" />
        <span className="score" title={`${plural(cast.likes, "like")}, ${plural(cast.recasts, "recast")}`}>
          {compactNumber(score)}
        </span>
        <span className="arrow down" aria-hidden="true" />
      </div>
      <Thumbnail cast={cast} />
      <div className="entry">
        <p className="title">
          <Link className="title" href={href} data-nav-primary="">
            {castTitle(cast)}
          </Link>{" "}
          <span className="domain">({domainLabel(cast)})</span>
        </p>
        <p className="tagline">
          submitted <TimeAgo iso={cast.timestamp} now={now} /> by{" "}
          <Link className="author" href={userHref(cast.author.username)}>
            @{cast.author.username}
          </Link>
          {cast.channel && (
            <>
              {" "}
              to{" "}
              <Link className="channel" href={channelHref(cast.channel)}>
                /c/{cast.channel.label}
              </Link>
            </>
          )}
          <span className="sep"> | </span>
          <Link className="comments" href={href}>
            {plural(cast.replies, "comment")}
          </Link>
        </p>
      </div>
    </div>
  );
}

export function PostList({ casts, offset = 0, now }: { casts: Cast[]; offset?: number; now: number }) {
  if (casts.length === 0) {
    return <p className="empty-listing">there doesn&apos;t seem to be anything here</p>;
  }
  return (
    <div id="siteTable" className="sitetable linklisting">
      {casts.map((cast, i) => (
        <PostRow key={cast.hash} cast={cast} rank={offset + i + 1} now={now} />
      ))}
    </div>
  );
}

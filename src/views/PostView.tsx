import Link from "next/link";
import { CastText } from "@/components/CastText";
import { CommentTree } from "@/components/CommentTree";
import { Embeds } from "@/components/Embeds";
import { PageBody } from "@/components/PageBody";
import { PostRow } from "@/components/PostRow";
import { ChannelBox, KeyboardHelpBox, PostInfoBox } from "@/components/SideBoxes";
import { SiteHeader } from "@/components/SiteHeader";
import { TimeAgo } from "@/components/TimeAgo";
import { visibleEmbeds } from "@/lib/embeds";
import { plural } from "@/lib/format";
import { channelHref, hrefFor, postHref, postRoute, userHref } from "@/lib/links";
import type { PostPageData, QuotedCastData } from "@/lib/queries";
import { castTitle, needsBody } from "@/lib/text";
import { COMMENT_SORTS, countNodes, sortThread, type CommentSort } from "@/lib/thread";
import type { Cast } from "@/lib/types";

/** "<title> : /c/<channel>", like old reddit's "<title> : <subreddit>". */
export function postTitle(cast: Cast): string {
  return `${castTitle(cast)} : ${cast.channel ? `/c/${cast.channel.label}` : "farcaster"}`;
}

function QuoteLink({ hash }: { hash: string }) {
  return (
    <p className="embed-link">
      <span className="embed-kind">quote</span> <Link href={postHref(hash)}>quoted cast {hash.slice(0, 10)}</Link>
    </p>
  );
}

function QuotedCast({ quote, now }: { quote: QuotedCastData; now: number }) {
  const quoted = quote.cast;
  if (!quoted) return <QuoteLink hash={quote.hash} />;
  return (
    <blockquote className="quoted-cast">
      <p className="tagline">
        quoting <Link href={userHref(quoted.author.username)}>@{quoted.author.username}</Link> ·{" "}
        <TimeAgo iso={quoted.timestamp} now={now} /> · <Link href={postHref(quoted.hash)}>view cast »</Link>
      </p>
      <CastText text={quoted.text} mentions={quoted.mentions} />
      <Embeds embeds={quoted.embeds.filter((embed) => embed.kind !== "cast")} text={quoted.text} compact />
    </blockquote>
  );
}

function ParentNotice({ hash, parent }: { hash: string; parent: Cast | null }) {
  return (
    <div className="infobar">
      you are viewing a single reply&apos;s thread.{" "}
      <Link href={postHref(hash)}>
        {parent ? `view the parent: “${castTitle(parent)}” by @${parent.author.username}` : "view the parent cast"}
      </Link>
    </div>
  );
}

export function PostView({ data, sort, now }: { data: PostPageData; sort: CommentSort; now: number }) {
  const { thread, channel, parent, quotes } = data;
  const root = thread.cast;
  const replies = sortThread(thread.children, sort);
  const loaded = countNodes(thread.children);
  const route = postRoute(root.hash);
  const media = visibleEmbeds(
    root.embeds.filter((embed) => embed.kind !== "cast"),
    root.text,
  );

  return (
    <>
      <SiteHeader
        pageName={root.channel?.label}
        pageHref={root.channel ? channelHref(root.channel) : undefined}
        tabs={[{ label: "comments", href: hrefFor(route), selected: true }]}
      />
      <PageBody
        side={
          <>
            <PostInfoBox cast={root} />
            {channel && <ChannelBox channel={channel} />}
            <KeyboardHelpBox />
          </>
        }
      >
        {root.parentHash && <ParentNotice hash={root.parentHash} parent={parent} />}
        <div id="siteTable" className="sitetable linklisting">
          <PostRow cast={root} now={now} />
        </div>
        {(needsBody(root.text) || media.length > 0 || quotes.length > 0) && (
          <div className="expando">
            <div className="usertext-body md">
              {needsBody(root.text) && <CastText text={root.text} mentions={root.mentions} />}
              <Embeds embeds={media} />
              {quotes.map((quote) => (
                <QuotedCast key={quote.hash} quote={quote} now={now} />
              ))}
            </div>
          </div>
        )}

        <div className="commentarea">
          <div className="panestack-title">
            <span className="title">
              {loaded >= root.replies
                ? `all ${plural(loaded, "comment")}`
                : `${plural(loaded, "comment")} shown of ${root.replies}`}
            </span>
          </div>
          <div className="menuarea">
            sorted by:{" "}
            {COMMENT_SORTS.map((option, i) => (
              <span key={option}>
                {i > 0 && " | "}
                <Link
                  className={option === sort ? "selected" : undefined}
                  href={hrefFor(route, { sort: option === "top" ? undefined : option })}
                >
                  {option}
                </Link>
              </span>
            ))}
          </div>
          {replies.length > 0 ? (
            <div className="sitetable nestedlisting">
              <CommentTree nodes={replies} opFid={root.author.fid} now={now} />
            </div>
          ) : (
            <p className="empty-listing">no comments (yet)</p>
          )}
        </div>
      </PageBody>
    </>
  );
}

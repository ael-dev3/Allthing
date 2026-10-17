import type { Metadata } from "next";
import Link from "next/link";
import { notFound } from "next/navigation";
import { Suspense } from "react";
import { CastText } from "@/components/CastText";
import { CommentTree } from "@/components/CommentTree";
import { Embeds } from "@/components/Embeds";
import { PageBody } from "@/components/PageBody";
import { PageFallback } from "@/components/PageFallback";
import { PostRow } from "@/components/PostRow";
import { ChannelBox, KeyboardHelpBox, PostInfoBox } from "@/components/SideBoxes";
import { SiteHeader } from "@/components/SiteHeader";
import { TimeAgo } from "@/components/TimeAgo";
import { channelHref } from "@/lib/channels";
import { visibleEmbeds } from "@/lib/embeds";
import { getCast, getChannel, getThread, requestNow } from "@/lib/data";
import { normalizeHash } from "@/lib/datasource/normalize";
import { plural } from "@/lib/format";
import { postHref, userHref } from "@/lib/links";
import { firstParam, queryString } from "@/lib/listing";
import { castTitle, needsBody, truncateTitle } from "@/lib/text";
import { COMMENT_SORTS, countNodes, parseCommentSort, sortThread } from "@/lib/thread";

const HASH_RE = /^(?:0x)?[0-9a-f]{40}$/i;

function hashFromParam(param: string): string | null {
  return HASH_RE.test(param.trim()) ? normalizeHash(param) : null;
}

export async function generateMetadata({ params }: PageProps<"/p/[hash]">): Promise<Metadata> {
  const hash = hashFromParam((await params).hash);
  const thread = hash ? await getThread(hash).catch(() => null) : null;
  if (!thread) return { title: "cast" };
  const { cast } = thread;
  return {
    title: `${castTitle(cast)} : ${cast.channel ? `/c/${cast.channel.label}` : "farcaster"}`,
    description: truncateTitle(cast.text, 200),
  };
}

export default function PostPage({ params, searchParams }: PageProps<"/p/[hash]">) {
  return (
    <Suspense fallback={<PageFallback />}>
      <PostWithComments params={params} searchParams={searchParams} />
    </Suspense>
  );
}

async function PostWithComments({ params, searchParams }: PageProps<"/p/[hash]">) {
  const [{ hash: param }, query] = await Promise.all([params, searchParams]);
  const hash = hashFromParam(param);
  if (!hash) notFound();
  const [thread, now] = await Promise.all([getThread(hash), requestNow()]);
  if (!thread) notFound();

  const root = thread.cast;
  const sort = parseCommentSort(firstParam(query.sort));
  const replies = sortThread(thread.children, sort);
  const loaded = countNodes(thread.children);
  const path = postHref(root.hash);
  const quotes = root.embeds.flatMap((embed) => (embed.kind === "cast" ? [embed.hash] : []));
  const media = visibleEmbeds(
    root.embeds.filter((embed) => embed.kind !== "cast"),
    root.text,
  );

  return (
    <>
      <SiteHeader
        pageName={root.channel?.label}
        pageHref={root.channel ? channelHref(root.channel) : undefined}
        tabs={[{ label: "comments", href: path, selected: true }]}
      />
      <PageBody
        side={
          <>
            <PostInfoBox cast={root} />
            {root.channel && (
              <Suspense fallback={null}>
                <ChannelSideBox channelKey={root.channel.id} />
              </Suspense>
            )}
            <KeyboardHelpBox />
          </>
        }
      >
        {root.parentHash && (
          <Suspense fallback={null}>
            <ParentNotice hash={root.parentHash} />
          </Suspense>
        )}
        <div id="siteTable" className="sitetable linklisting">
          <PostRow cast={root} now={now} />
        </div>
        {(needsBody(root.text) || media.length > 0 || quotes.length > 0) && (
          <div className="expando">
            <div className="usertext-body md">
              {needsBody(root.text) && <CastText text={root.text} mentions={root.mentions} />}
              <Embeds embeds={media} />
              {quotes.map((quoted) => (
                <Suspense key={quoted} fallback={<QuoteLink hash={quoted} />}>
                  <QuotedCast hash={quoted} now={now} />
                </Suspense>
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
                  href={path + queryString({ sort: option === "top" ? undefined : option })}
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

async function ChannelSideBox({ channelKey }: { channelKey: string }) {
  const channel = await getChannel(channelKey).catch(() => null);
  return channel ? <ChannelBox channel={channel} /> : null;
}

async function ParentNotice({ hash }: { hash: string }) {
  const parent = await getCast(hash).catch(() => null);
  return (
    <div className="infobar">
      you are viewing a single reply&apos;s thread.{" "}
      <Link href={postHref(hash)}>
        {parent ? `view the parent: “${castTitle(parent)}” by @${parent.author.username}` : "view the parent cast"}
      </Link>
    </div>
  );
}

function QuoteLink({ hash }: { hash: string }) {
  return (
    <p className="embed-link">
      <span className="embed-kind">quote</span> <Link href={postHref(hash)}>quoted cast {hash.slice(0, 10)}</Link>
    </p>
  );
}

async function QuotedCast({ hash, now }: { hash: string; now: number }) {
  const quoted = await getCast(hash).catch(() => null);
  if (!quoted) return <QuoteLink hash={hash} />;
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

import Link from "next/link";
import { plural } from "@/lib/format";
import { postHref, userHref } from "@/lib/links";
import { castScore } from "@/lib/ranking";
import { countDescendants } from "@/lib/thread";
import type { ThreadNode } from "@/lib/types";
import { CastText } from "./CastText";
import { Embeds } from "./Embeds";
import { TimeAgo } from "./TimeAgo";

function commentId(hash: string): string {
  return `c-${hash.slice(2, 12)}`;
}

function Comment({
  node,
  opFid,
  parentHash,
  now,
}: {
  node: ThreadNode;
  opFid: number;
  parentHash: string | null;
  now: number;
}) {
  const { cast, children } = node;
  const descendants = countDescendants(node);
  const unloaded = Math.max(0, cast.replies - children.length);
  const isOp = cast.author.fid === opFid;

  return (
    <details open className="thing comment" id={commentId(cast.hash)} data-nav="">
      <summary className="tagline">
        <span className="expand" aria-hidden="true" />
        <Link className={isOp ? "author submitter" : "author"} href={userHref(cast.author.username)}>
          @{cast.author.username}
        </Link>
        {isOp && (
          <span className="userattrs">
            [<span className="submitter" title="submitter">S</span>]
          </span>
        )}{" "}
        <span className="score">{plural(castScore(cast), "point")}</span>{" "}
        <TimeAgo iso={cast.timestamp} now={now} />{" "}
        <span className="numchildren">({plural(descendants, "child", "children")})</span>
      </summary>
      <div className="entry">
        <div className="usertext-body md">
          {cast.text.trim() ? <CastText text={cast.text} mentions={cast.mentions} /> : null}
          <Embeds embeds={cast.embeds} text={cast.text} compact />
        </div>
        <ul className="flat-list buttons">
          <li>
            <Link href={postHref(cast.hash)}>permalink</Link>
          </li>
          {parentHash && (
            <li>
              <a href={`#${commentId(parentHash)}`}>parent</a>
            </li>
          )}
        </ul>
      </div>
      {children.length > 0 && (
        <div className="child">
          <CommentTree nodes={children} opFid={opFid} parentHash={cast.hash} now={now} />
        </div>
      )}
      {unloaded > 0 && (
        <div className="child morechildren">
          <Link href={postHref(cast.hash)}>continue this thread →</Link>{" "}
          <span className="gray">({plural(unloaded, "more reply", "more replies")})</span>
        </div>
      )}
    </details>
  );
}

/**
 * Threaded replies. Each comment is a <details> element, so collapsing works
 * without JavaScript; [–]/[+] toggles come from CSS.
 */
export function CommentTree({
  nodes,
  opFid,
  parentHash = null,
  now,
}: {
  nodes: ThreadNode[];
  opFid: number;
  parentHash?: string | null;
  now: number;
}) {
  return (
    <>
      {nodes.map((node) => (
        <Comment key={node.cast.hash} node={node} opFid={opFid} parentHash={parentHash} now={now} />
      ))}
    </>
  );
}

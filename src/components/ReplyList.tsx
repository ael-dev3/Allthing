import Link from "next/link";
import { plural } from "@/lib/format";
import { channelHref, postHref, userHref } from "@/lib/links";
import { castScore } from "@/lib/ranking";
import type { Cast } from "@/lib/types";
import { CastText } from "./CastText";
import { Embeds } from "./Embeds";
import { TimeAgo } from "./TimeAgo";

/** A user's replies, shown like the comments tab of an old reddit profile. */
export function ReplyList({ casts, now }: { casts: Cast[]; now: number }) {
  if (casts.length === 0) {
    return <p className="empty-listing">no replies here</p>;
  }
  return (
    <div className="sitetable reply-listing">
      {casts.map((cast) => (
        <div key={cast.hash} className="thing reply-entry" data-nav="">
          <p className="parent-line">
            <Link href={postHref(cast.parentHash ?? cast.hash)} data-nav-primary="">
              reply to a cast
            </Link>
            {cast.channel && (
              <>
                {" "}
                in{" "}
                <Link className="channel" href={channelHref(cast.channel)}>
                  /c/{cast.channel.label}
                </Link>
              </>
            )}
          </p>
          <div className="reply-body">
            <p className="tagline">
              <Link className="author" href={userHref(cast.author.username)}>
                @{cast.author.username}
              </Link>{" "}
              <span className="score">{plural(castScore(cast), "point")}</span>{" "}
              <TimeAgo iso={cast.timestamp} now={now} />
            </p>
            <div className="usertext-body md">
              {cast.text.trim() && <CastText text={cast.text} mentions={cast.mentions} />}
              <Embeds embeds={cast.embeds} text={cast.text} compact />
            </div>
            <ul className="flat-list buttons">
              <li>
                <Link href={postHref(cast.hash)}>permalink</Link>
              </li>
              {cast.parentHash && (
                <li>
                  <Link href={postHref(cast.parentHash)}>context</Link>
                </li>
              )}
              <li>
                <span className="gray">{plural(cast.replies, "reply", "replies")}</span>
              </li>
            </ul>
          </div>
        </div>
      ))}
    </div>
  );
}

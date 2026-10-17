import Link from "next/link";
import { channelHref, channelLabel, isChannelId } from "@/lib/channels";
import { getTrendingChannels } from "@/lib/data";
import { resizedImageUrl } from "@/lib/embeds";
import { fullNumber, plural } from "@/lib/format";
import { farcasterCastUrl, userHref } from "@/lib/links";
import { castScore } from "@/lib/ranking";
import { formatDate } from "@/lib/time";
import type { Cast, Channel, User } from "@/lib/types";
import { CastText } from "./CastText";
import { SideBox } from "./PageBody";

export function AboutBox({ sourceName }: { sourceName: string }) {
  return (
    <SideBox title="about allthing">
      <div className="md">
        <p>
          A read-only, old-reddit-style client for <a href="https://farcaster.xyz">Farcaster</a>. The front page ranks
          trending casts; channels work like subreddits.
        </p>
        <p>
          No login, no posting. Data from <strong>{sourceName}</strong>.
        </p>
      </div>
    </SideBox>
  );
}

export function KeyboardHelpBox() {
  return (
    <SideBox title="keyboard shortcuts">
      <table className="kbd-help">
        <tbody>
          <tr>
            <td>
              <kbd>j</kbd> <kbd>k</kbd>
            </td>
            <td>next / previous item</td>
          </tr>
          <tr>
            <td>
              <kbd>o</kbd> <kbd>enter</kbd>
            </td>
            <td>open post</td>
          </tr>
          <tr>
            <td>
              <kbd>x</kbd>
            </td>
            <td>collapse / expand comment</td>
          </tr>
          <tr>
            <td>
              <kbd>esc</kbd>
            </td>
            <td>clear selection</td>
          </tr>
        </tbody>
      </table>
    </SideBox>
  );
}

export async function TrendingChannelsBox() {
  let channels: Channel[] = [];
  try {
    channels = await getTrendingChannels();
  } catch {
    return null; // the sidebar is optional; the listing still renders
  }
  if (channels.length === 0) return null;
  return (
    <SideBox title="trending channels">
      <ol className="channel-list">
        {channels.map((channel) => (
          <li key={channel.id}>
            <Link href={channelHref(channel)}>/c/{channelLabel(channel.id)}</Link>
            {channel.memberCount !== null && (
              <span className="gray"> · {plural(channel.memberCount, "member")}</span>
            )}
          </li>
        ))}
      </ol>
      <p className="more-link">
        <Link href="/channels">all channels »</Link>
      </p>
    </SideBox>
  );
}

export function ChannelBox({ channel }: { channel: Channel }) {
  return (
    <section className="spacer titlebox">
      <h1 className="redditname">
        <Link href={channelHref(channel)}>{channel.name}</Link>
      </h1>
      {channel.memberCount !== null && (
        <p className="subscribers">
          <span className="number">{fullNumber(channel.memberCount)}</span>{" "}
          <span className="word">{channel.memberCount === 1 ? "member" : "members"}</span>
        </p>
      )}
      {channel.description && (
        <div className="md">
          <CastText text={channel.description} />
        </div>
      )}
      <p className="parent-url">
        parent url: <code>{channel.parentUrl}</code>
      </p>
      {isChannelId(channel.id) && (
        <p>
          <a href={`https://farcaster.xyz/~/channel/${channel.id}`}>open on farcaster.xyz »</a>
        </p>
      )}
    </section>
  );
}

export function UserBox({ user }: { user: User }) {
  return (
    <section className="spacer titlebox userbox">
      {user.pfpUrl && (
        // eslint-disable-next-line @next/next/no-img-element -- remote, arbitrary hosts
        <img
          className="pfp"
          src={resizedImageUrl(user.pfpUrl, "thumb")}
          alt=""
          width={64}
          height={64}
          referrerPolicy="no-referrer"
        />
      )}
      <h1 className="redditname">
        <Link href={userHref(user.username)}>{user.displayName || user.username}</Link>
      </h1>
      <p className="gray">
        @{user.username} · fid {user.fid}
      </p>
      <p className="karma">
        <span className="number">{fullNumber(user.followerCount)}</span> followers ·{" "}
        <span className="number">{fullNumber(user.followingCount)}</span> following
      </p>
      {user.bio && (
        <div className="md bio">
          <CastText text={user.bio} />
        </div>
      )}
      {user.verifiedAddresses.length > 0 && (
        <p className="gray">{plural(user.verifiedAddresses.length, "verified address", "verified addresses")}</p>
      )}
      {!user.username.startsWith("fid:") && (
        <p>
          <a href={`https://farcaster.xyz/${encodeURIComponent(user.username)}`}>view on farcaster.xyz »</a>
        </p>
      )}
    </section>
  );
}

export function PostInfoBox({ cast }: { cast: Cast }) {
  return (
    <SideBox title="this cast">
      <p>
        submitted on <time dateTime={cast.timestamp}>{formatDate(cast.timestamp)}</time>
      </p>
      <p>
        <span className="number">{plural(castScore(cast), "point")}</span>{" "}
        <span className="gray">
          ({plural(cast.likes, "like")}, {plural(cast.recasts, "recast")})
        </span>
      </p>
      <p className="gray">
        hash <code>{cast.hash.slice(0, 12)}…</code>
      </p>
      {!cast.author.username.startsWith("fid:") && (
        <p>
          <a href={farcasterCastUrl(cast.author.username, cast.hash)}>view on farcaster.xyz »</a>
        </p>
      )}
    </SideBox>
  );
}

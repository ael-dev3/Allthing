import { resizedImageUrl } from "@/lib/embeds";
import { fullNumber } from "@/lib/format";
import type { User } from "@/lib/types";
import { CastText } from "./CastText";

/** The top of a profile page: picture, display name, handle, follower counts and bio. */
export function ProfileHeader({ user }: { user: User }) {
  return (
    <section className="profile-header" aria-label="profile">
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
      <div className="profile-info">
        <h1 className="display-name">{user.displayName || user.username}</h1>
        <p className="handle">
          @{user.username} · fid {user.fid}
        </p>
        <p className="counts">
          <span className="followers">
            <span className="number">{fullNumber(user.followerCount)}</span> followers
          </span>{" "}
          ·{" "}
          <span className="following">
            <span className="number">{fullNumber(user.followingCount)}</span> following
          </span>
        </p>
        {user.bio.trim() ? (
          <div className="md bio">
            <CastText text={user.bio} />
          </div>
        ) : (
          <p className="bio gray">no bio</p>
        )}
      </div>
    </section>
  );
}

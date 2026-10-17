import Link from "next/link";
import { NextPrev } from "@/components/NextPrev";
import { PageBody } from "@/components/PageBody";
import { PostList } from "@/components/PostRow";
import { ReplyList } from "@/components/ReplyList";
import { KeyboardHelpBox, UserBox } from "@/components/SideBoxes";
import { SiteHeader } from "@/components/SiteHeader";
import { hrefFor, userRoute } from "@/lib/links";
import { paginate, repliesOnly, topLevelOnly } from "@/lib/listing";
import type { UserPageData } from "@/lib/queries";
import { SORTS, sortCasts, type SortKey } from "@/lib/ranking";

export type UserTab = "posts" | "replies";

export function parseUserTab(value: string | null | undefined): UserTab {
  return value === "replies" ? "replies" : "posts";
}

export function userTitle(username: string): string {
  return `overview for @${username}`;
}

export function UserView({
  data,
  tab,
  sort,
  page,
  now,
}: {
  data: UserPageData;
  tab: UserTab;
  sort: SortKey;
  page: number;
  now: number;
}) {
  const { user, pool } = data;
  const route = userRoute(user.username);
  const posts = paginate(sortCasts(topLevelOnly(pool), sort), page);
  const replies = paginate(repliesOnly(pool), page);

  return (
    <>
      <SiteHeader
        pageName={`@${user.username}`}
        pageHref={hrefFor(route)}
        tabs={[
          { label: "posts", href: hrefFor(route), selected: tab === "posts" },
          { label: "replies", href: hrefFor(route, { tab: "replies" }), selected: tab === "replies" },
        ]}
      />
      <PageBody
        side={
          <>
            <UserBox user={user} />
            <KeyboardHelpBox />
          </>
        }
      >
        {tab === "posts" ? (
          <>
            <div className="menuarea">
              sorted by:{" "}
              {SORTS.map((option, i) => (
                <span key={option}>
                  {i > 0 && " | "}
                  <Link
                    className={option === sort ? "selected" : undefined}
                    href={hrefFor(route, { sort: option === "new" ? undefined : option })}
                  >
                    {option}
                  </Link>
                </span>
              ))}
            </div>
            <PostList casts={posts.items} offset={posts.offset} now={now} />
            <NextPrev route={route} params={{ sort: sort === "new" ? undefined : sort }} slice={posts} />
          </>
        ) : (
          <>
            <ReplyList casts={replies.items} now={now} />
            <NextPrev route={route} params={{ tab: "replies" }} slice={replies} />
          </>
        )}
      </PageBody>
    </>
  );
}

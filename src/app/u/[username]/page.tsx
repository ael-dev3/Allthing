import type { Metadata } from "next";
import Link from "next/link";
import { notFound } from "next/navigation";
import { Suspense } from "react";
import { NextPrev } from "@/components/NextPrev";
import { PageBody } from "@/components/PageBody";
import { PageFallback } from "@/components/PageFallback";
import { PostList } from "@/components/PostRow";
import { ReplyList } from "@/components/ReplyList";
import { KeyboardHelpBox, UserBox } from "@/components/SideBoxes";
import { SiteHeader } from "@/components/SiteHeader";
import { getUser, getUserPool, requestNow } from "@/lib/data";
import { userHref } from "@/lib/links";
import { firstParam, paginate, parsePage, queryString, repliesOnly, topLevelOnly } from "@/lib/listing";
import { SORTS, parseSort, sortCasts } from "@/lib/ranking";

const USERNAME_RE = /^(?:fid:\d+|[a-z0-9][a-z0-9._-]{0,63})$/;

function usernameFromParam(param: string): string | null {
  let value = param;
  if (value.includes("%")) {
    try {
      value = decodeURIComponent(value);
    } catch {
      return null;
    }
  }
  value = value.trim().replace(/^@/, "").toLowerCase();
  return USERNAME_RE.test(value) ? value : null;
}

export async function generateMetadata({ params }: PageProps<"/u/[username]">): Promise<Metadata> {
  const username = usernameFromParam((await params).username);
  return { title: username ? `overview for @${username}` : "user not found" };
}

export default function UserPage({ params, searchParams }: PageProps<"/u/[username]">) {
  return (
    <Suspense fallback={<PageFallback />}>
      <UserProfile params={params} searchParams={searchParams} />
    </Suspense>
  );
}

async function UserProfile({ params, searchParams }: PageProps<"/u/[username]">) {
  const [{ username: param }, query] = await Promise.all([params, searchParams]);
  const username = usernameFromParam(param);
  if (!username) notFound();
  const user = await getUser(username);
  if (!user) notFound();

  const tab = firstParam(query.tab) === "replies" ? "replies" : "posts";
  const sort = parseSort(firstParam(query.sort), "new");
  const page = parsePage(firstParam(query.page));
  const [pool, now] = await Promise.all([getUserPool(user.fid), requestNow()]);
  const path = userHref(user.username);

  const posts = paginate(sortCasts(topLevelOnly(pool), sort), page);
  const replies = paginate(repliesOnly(pool), page);

  return (
    <>
      <SiteHeader
        pageName={`@${user.username}`}
        pageHref={path}
        tabs={[
          { label: "posts", href: path, selected: tab === "posts" },
          { label: "replies", href: `${path}?tab=replies`, selected: tab === "replies" },
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
                    href={path + queryString({ sort: option === "new" ? undefined : option })}
                  >
                    {option}
                  </Link>
                </span>
              ))}
            </div>
            <PostList casts={posts.items} offset={posts.offset} now={now} />
            <NextPrev path={path} params={{ sort: sort === "new" ? undefined : sort }} slice={posts} />
          </>
        ) : (
          <>
            <ReplyList casts={replies.items} now={now} />
            <NextPrev path={path} params={{ tab: "replies" }} slice={replies} />
          </>
        )}
      </PageBody>
    </>
  );
}

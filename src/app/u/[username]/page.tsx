import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { Suspense } from "react";
import { PageFallback } from "@/components/PageFallback";
import { getUserPage, requestNow } from "@/lib/data";
import { firstParam, parsePage } from "@/lib/listing";
import { usernameFromParam } from "@/lib/params";
import { parseSort } from "@/lib/ranking";
import type { RouteProps } from "@/lib/route-props";
import { UserView, parseUserTab, userTitle } from "@/views/UserView";

type Props = RouteProps<{ username: string }>;

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const username = usernameFromParam((await params).username);
  return { title: username ? userTitle(username) : "user not found" };
}

export default function UserPage({ params, searchParams }: Props) {
  return (
    <Suspense fallback={<PageFallback />}>
      <UserProfile params={params} searchParams={searchParams} />
    </Suspense>
  );
}

async function UserProfile({ params, searchParams }: Props) {
  const [{ username: param }, query] = await Promise.all([params, searchParams]);
  const username = usernameFromParam(param);
  if (!username) notFound();
  const [data, now] = await Promise.all([getUserPage(username), requestNow()]);
  if (!data) notFound();
  return (
    <UserView
      data={data}
      tab={parseUserTab(firstParam(query.tab))}
      sort={parseSort(firstParam(query.sort), "new")}
      page={parsePage(firstParam(query.page))}
      now={now}
    />
  );
}

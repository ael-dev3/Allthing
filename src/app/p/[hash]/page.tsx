import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { Suspense } from "react";
import { PageFallback } from "@/components/PageFallback";
import { getPostPage, requestNow } from "@/lib/data";
import { firstParam } from "@/lib/listing";
import { hashFromParam } from "@/lib/params";
import type { RouteProps } from "@/lib/route-props";
import { truncateTitle } from "@/lib/text";
import { parseCommentSort } from "@/lib/thread";
import { PostView, postTitle } from "@/views/PostView";

type Props = RouteProps<{ hash: string }>;

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const hash = hashFromParam((await params).hash);
  const data = hash ? await getPostPage(hash).catch(() => null) : null;
  if (!data) return { title: "cast" };
  return { title: postTitle(data.thread.cast), description: truncateTitle(data.thread.cast.text, 200) };
}

export default function PostPage({ params, searchParams }: Props) {
  return (
    <Suspense fallback={<PageFallback />}>
      <PostWithComments params={params} searchParams={searchParams} />
    </Suspense>
  );
}

async function PostWithComments({ params, searchParams }: Props) {
  const [{ hash: param }, query] = await Promise.all([params, searchParams]);
  const hash = hashFromParam(param);
  if (!hash) notFound();
  const [data, now] = await Promise.all([getPostPage(hash), requestNow()]);
  if (!data) notFound();
  return <PostView data={data} sort={parseCommentSort(firstParam(query.sort))} now={now} />;
}

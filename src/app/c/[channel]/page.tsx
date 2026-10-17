import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { Suspense } from "react";
import { PageFallback } from "@/components/PageFallback";
import { channelKeyFromParam } from "@/lib/channels";
import { getChannelPage, requestNow } from "@/lib/data";
import { firstParam, parsePage } from "@/lib/listing";
import { parseSort } from "@/lib/ranking";
import type { RouteProps } from "@/lib/route-props";
import { ChannelView, channelTitle } from "@/views/ChannelView";

type Props = RouteProps<{ channel: string }>;

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const key = channelKeyFromParam((await params).channel);
  return { title: key ? channelTitle(key) : "channel not found" };
}

export default function ChannelPage({ params, searchParams }: Props) {
  return (
    <Suspense fallback={<PageFallback />}>
      <ChannelListing params={params} searchParams={searchParams} />
    </Suspense>
  );
}

async function ChannelListing({ params, searchParams }: Props) {
  const [{ channel: param }, query] = await Promise.all([params, searchParams]);
  const key = channelKeyFromParam(param);
  if (!key) notFound();
  const [data, now] = await Promise.all([getChannelPage(key), requestNow()]);
  if (!data) notFound();
  return (
    <ChannelView
      data={data}
      sort={parseSort(firstParam(query.sort))}
      page={parsePage(firstParam(query.page))}
      now={now}
    />
  );
}

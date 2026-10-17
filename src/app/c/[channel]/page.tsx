import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { Suspense } from "react";
import { NextPrev } from "@/components/NextPrev";
import { PageBody } from "@/components/PageBody";
import { PageFallback } from "@/components/PageFallback";
import { PostList } from "@/components/PostRow";
import { ChannelBox, KeyboardHelpBox } from "@/components/SideBoxes";
import { SiteHeader } from "@/components/SiteHeader";
import { channelHref, channelKeyFromParam, channelLabel } from "@/lib/channels";
import { getChannel, getChannelPool, requestNow } from "@/lib/data";
import { sortTabs } from "@/lib/links";
import { firstParam, paginate, parsePage, topLevelOnly } from "@/lib/listing";
import { parseSort, sortCasts } from "@/lib/ranking";

export async function generateMetadata({ params }: PageProps<"/c/[channel]">): Promise<Metadata> {
  const key = channelKeyFromParam((await params).channel);
  return { title: key ? `/c/${channelLabel(key)}` : "channel not found" };
}

export default function ChannelPage({ params, searchParams }: PageProps<"/c/[channel]">) {
  return (
    <Suspense fallback={<PageFallback />}>
      <ChannelListing params={params} searchParams={searchParams} />
    </Suspense>
  );
}

async function ChannelListing({ params, searchParams }: PageProps<"/c/[channel]">) {
  const [{ channel: param }, query] = await Promise.all([params, searchParams]);
  const key = channelKeyFromParam(param);
  if (!key) notFound();
  const channel = await getChannel(key);
  if (!channel) notFound();

  const sort = parseSort(firstParam(query.sort));
  const page = parsePage(firstParam(query.page));
  const [pool, now] = await Promise.all([getChannelPool(channel.parentUrl), requestNow()]);
  const slice = paginate(sortCasts(topLevelOnly(pool), sort), page);
  const path = channelHref(channel);

  return (
    <>
      <SiteHeader pageName={channelLabel(channel.id)} pageHref={path} tabs={sortTabs(path, sort)} />
      <PageBody
        side={
          <>
            <ChannelBox channel={channel} />
            <KeyboardHelpBox />
          </>
        }
      >
        <PostList casts={slice.items} offset={slice.offset} now={now} />
        <NextPrev path={path} params={{ sort: sort === "hot" ? undefined : sort }} slice={slice} />
      </PageBody>
    </>
  );
}

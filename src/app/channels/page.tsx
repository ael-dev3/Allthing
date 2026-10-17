import type { Metadata } from "next";
import { Suspense } from "react";
import { PageFallback } from "@/components/PageFallback";
import { getChannelDirectory } from "@/lib/data";
import { firstParam } from "@/lib/listing";
import type { SearchParams } from "@/lib/route-props";
import { ChannelsView } from "@/views/ChannelsView";

export const metadata: Metadata = { title: "channels" };

export default function ChannelsPage({ searchParams }: { searchParams: SearchParams }) {
  return (
    <Suspense fallback={<PageFallback pageName="channels" />}>
      <ChannelDirectory searchParams={searchParams} />
    </Suspense>
  );
}

async function ChannelDirectory({ searchParams }: { searchParams: SearchParams }) {
  const query = firstParam((await searchParams).q)?.trim() ?? "";
  return <ChannelsView query={query} channels={await getChannelDirectory(query)} />;
}

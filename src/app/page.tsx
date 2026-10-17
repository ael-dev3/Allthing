import type { Metadata } from "next";
import { Suspense } from "react";
import { PageFallback } from "@/components/PageFallback";
import { getFrontPage, requestNow } from "@/lib/data";
import { getDataSource } from "@/lib/datasource";
import { firstParam, parsePage } from "@/lib/listing";
import { parseSort } from "@/lib/ranking";
import type { SearchParams } from "@/lib/route-props";
import { FrontPageView } from "@/views/FrontPageView";

export const metadata: Metadata = {
  title: "allthing: the front page of farcaster",
};

export default function FrontPage({ searchParams }: { searchParams: SearchParams }) {
  return (
    <Suspense fallback={<PageFallback />}>
      <FrontPageListing searchParams={searchParams} />
    </Suspense>
  );
}

async function FrontPageListing({ searchParams }: { searchParams: SearchParams }) {
  const query = await searchParams;
  const [data, now] = await Promise.all([getFrontPage(), requestNow()]);
  return (
    <FrontPageView
      data={data}
      sort={parseSort(firstParam(query.sort))}
      page={parsePage(firstParam(query.page))}
      now={now}
      sourceName={getDataSource().name}
    />
  );
}

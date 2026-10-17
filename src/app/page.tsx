import type { Metadata } from "next";
import { Suspense } from "react";
import { NextPrev } from "@/components/NextPrev";
import { PageBody } from "@/components/PageBody";
import { PageFallback } from "@/components/PageFallback";
import { PostList } from "@/components/PostRow";
import { AboutBox, KeyboardHelpBox, TrendingChannelsBox } from "@/components/SideBoxes";
import { SiteHeader } from "@/components/SiteHeader";
import { getTrendingPool, requestNow } from "@/lib/data";
import { getDataSource } from "@/lib/datasource";
import { sortTabs } from "@/lib/links";
import { firstParam, paginate, parsePage, topLevelOnly } from "@/lib/listing";
import { parseSort, sortCasts } from "@/lib/ranking";

export const metadata: Metadata = {
  title: "allthing: the front page of farcaster",
};

export default function FrontPage({ searchParams }: PageProps<"/">) {
  return (
    <Suspense fallback={<PageFallback />}>
      <FrontPageListing searchParams={searchParams} />
    </Suspense>
  );
}

async function FrontPageListing({ searchParams }: Pick<PageProps<"/">, "searchParams">) {
  const query = await searchParams;
  const sort = parseSort(firstParam(query.sort));
  const page = parsePage(firstParam(query.page));
  const [pool, now] = await Promise.all([getTrendingPool(), requestNow()]);
  const slice = paginate(sortCasts(topLevelOnly(pool), sort), page);

  return (
    <>
      <SiteHeader tabs={sortTabs("/", sort)} />
      <PageBody
        side={
          <>
            <AboutBox sourceName={getDataSource().name} />
            <Suspense fallback={null}>
              <TrendingChannelsBox />
            </Suspense>
            <KeyboardHelpBox />
          </>
        }
      >
        <PostList casts={slice.items} offset={slice.offset} now={now} />
        <NextPrev path="/" params={{ sort: sort === "hot" ? undefined : sort }} slice={slice} />
      </PageBody>
    </>
  );
}

import { NextPrev } from "@/components/NextPrev";
import { PageBody } from "@/components/PageBody";
import { PostList } from "@/components/PostRow";
import { AboutBox, KeyboardHelpBox, TrendingChannelsBox } from "@/components/SideBoxes";
import { SiteHeader } from "@/components/SiteHeader";
import { homeRoute, sortTabs } from "@/lib/links";
import { paginate, topLevelOnly } from "@/lib/listing";
import type { FrontPageData } from "@/lib/queries";
import { sortCasts, type SortKey } from "@/lib/ranking";

export function FrontPageView({
  data,
  sort,
  page,
  now,
  sourceName,
}: {
  data: FrontPageData;
  sort: SortKey;
  page: number;
  now: number;
  sourceName: string;
}) {
  const slice = paginate(sortCasts(topLevelOnly(data.pool), sort), page);
  return (
    <>
      <SiteHeader tabs={sortTabs(homeRoute, sort)} />
      <PageBody
        side={
          <>
            <AboutBox sourceName={sourceName} />
            <TrendingChannelsBox channels={data.trendingChannels} />
            <KeyboardHelpBox />
          </>
        }
      >
        <PostList casts={slice.items} offset={slice.offset} now={now} />
        <NextPrev route={homeRoute} params={{ sort: sort === "hot" ? undefined : sort }} slice={slice} />
      </PageBody>
    </>
  );
}

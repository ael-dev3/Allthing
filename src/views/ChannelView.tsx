import { NextPrev } from "@/components/NextPrev";
import { PageBody } from "@/components/PageBody";
import { PostList } from "@/components/PostRow";
import { ChannelBox, KeyboardHelpBox } from "@/components/SideBoxes";
import { SiteHeader } from "@/components/SiteHeader";
import { channelLabel } from "@/lib/channels";
import { channelRoute, hrefFor, sortTabs } from "@/lib/links";
import { paginate, topLevelOnly } from "@/lib/listing";
import type { ChannelPageData } from "@/lib/queries";
import { sortCasts, type SortKey } from "@/lib/ranking";

export function channelTitle(channelId: string): string {
  return `/c/${channelLabel(channelId)}`;
}

export function ChannelView({
  data,
  sort,
  page,
  now,
}: {
  data: ChannelPageData;
  sort: SortKey;
  page: number;
  now: number;
}) {
  const { channel, pool } = data;
  const route = channelRoute(channel);
  const slice = paginate(sortCasts(topLevelOnly(pool), sort), page);
  return (
    <>
      <SiteHeader pageName={channelLabel(channel.id)} pageHref={hrefFor(route)} tabs={sortTabs(route, sort)} />
      <PageBody
        side={
          <>
            <ChannelBox channel={channel} />
            <KeyboardHelpBox />
          </>
        }
      >
        <PostList casts={slice.items} offset={slice.offset} now={now} />
        <NextPrev route={route} params={{ sort: sort === "hot" ? undefined : sort }} slice={slice} />
      </PageBody>
    </>
  );
}

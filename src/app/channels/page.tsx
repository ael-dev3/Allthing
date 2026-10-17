import type { Metadata } from "next";
import Form from "next/form";
import Link from "next/link";
import { Suspense } from "react";
import { PageBody, SideBox } from "@/components/PageBody";
import { PageFallback } from "@/components/PageFallback";
import { KeyboardHelpBox } from "@/components/SideBoxes";
import { SiteHeader } from "@/components/SiteHeader";
import { DEFAULT_CHANNELS, channelHref, channelLabel } from "@/lib/channels";
import { getTrendingChannels, searchChannels } from "@/lib/data";
import { plural } from "@/lib/format";
import { firstParam } from "@/lib/listing";
import type { Channel } from "@/lib/types";

export const metadata: Metadata = { title: "channels" };

export default function ChannelsPage({ searchParams }: PageProps<"/channels">) {
  return (
    <Suspense fallback={<PageFallback pageName="channels" />}>
      <ChannelDirectory searchParams={searchParams} />
    </Suspense>
  );
}

async function ChannelDirectory({ searchParams }: Pick<PageProps<"/channels">, "searchParams">) {
  const query = firstParam((await searchParams).q)?.trim() ?? "";
  const channels: Channel[] = query ? await searchChannels(query) : await getTrendingChannels(50);

  return (
    <>
      <SiteHeader pageName="channels" pageHref="/channels" />
      <PageBody
        side={
          <>
            <SideBox title="popular channels">
              <ul className="channel-list">
                {DEFAULT_CHANNELS.map((id) => (
                  <li key={id}>
                    <Link href={channelHref(id)}>/c/{id}</Link>
                  </li>
                ))}
              </ul>
            </SideBox>
            <KeyboardHelpBox />
          </>
        }
      >
        <Form action="/channels" className="search-form">
          <input name="q" type="search" defaultValue={query} placeholder="search channels" aria-label="search channels" />{" "}
          <button type="submit">search</button>
        </Form>
        <h2 className="listing-heading">
          {query ? `channels matching “${query}”` : "trending channels"}
        </h2>
        {channels.length === 0 ? (
          <p className="empty-listing">no channels found</p>
        ) : (
          <div className="sitetable channel-listing">
            {channels.map((channel) => (
              <div key={channel.id} className="thing channel-entry" data-nav="">
                <p className="title">
                  <Link className="title" href={channelHref(channel)} data-nav-primary="">
                    /c/{channelLabel(channel.id)}
                  </Link>
                </p>
                <p className="tagline">
                  {channel.memberCount !== null ? plural(channel.memberCount, "member") : "member count unknown"}
                  {channel.description && <> · {channel.description}</>}
                </p>
              </div>
            ))}
          </div>
        )}
      </PageBody>
    </>
  );
}

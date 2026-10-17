import Form from "next/form";
import Link from "next/link";
import { PageBody, SideBox } from "@/components/PageBody";
import { KeyboardHelpBox } from "@/components/SideBoxes";
import { SiteHeader } from "@/components/SiteHeader";
import { DEFAULT_CHANNELS, channelLabel } from "@/lib/channels";
import { plural } from "@/lib/format";
import { channelHref, channelsRoute, hrefFor } from "@/lib/links";
import type { Channel } from "@/lib/types";

export function ChannelsView({ query, channels }: { query: string; channels: Channel[] }) {
  return (
    <>
      <SiteHeader pageName="channels" pageHref={hrefFor(channelsRoute)} />
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
        {/* An empty action submits to this same page, whichever URL scheme is in use. */}
        <Form action="" className="search-form">
          <input name="q" type="search" defaultValue={query} placeholder="search channels" aria-label="search channels" />{" "}
          <button type="submit">search</button>
        </Form>
        <h2 className="listing-heading">{query ? `channels matching “${query}”` : "trending channels"}</h2>
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

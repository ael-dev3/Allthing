import Link from "next/link";
import { Fragment } from "react";
import { DEFAULT_CHANNELS } from "@/lib/channels";
import { channelHref, channelsRoute, hrefFor, homeRoute, type Tab } from "@/lib/links";
import { Logo } from "./Logo";

function ChannelStrip() {
  return (
    <div id="sr-header-area">
      <nav className="sr-list" aria-label="channels">
        <Link href={hrefFor(homeRoute)}>front</Link>
        <span className="separator">-</span>
        <Link href={hrefFor(channelsRoute)}>channels</Link>
        <span className="separator">|</span>
        {DEFAULT_CHANNELS.map((id, i) => (
          <Fragment key={id}>
            {i > 0 && <span className="separator">-</span>}
            <Link href={channelHref(id)}>{id}</Link>
          </Fragment>
        ))}
      </nav>
      <Link className="sr-more" href={hrefFor(channelsRoute)}>
        more »
      </Link>
    </div>
  );
}

export function SiteHeader({
  pageName,
  pageHref,
  tabs = [],
}: {
  pageName?: string;
  pageHref?: string;
  tabs?: Tab[];
}) {
  return (
    <header id="header">
      <ChannelStrip />
      <div id="header-bottom">
        <div id="header-bottom-left">
          <Link href={hrefFor(homeRoute)} id="header-img" aria-label="allthing front page">
            <Logo />
            <span className="logo-text">allthing</span>
          </Link>
          {pageName && (
            <span className="pagename">
              {pageHref ? <Link href={pageHref}>{pageName}</Link> : pageName}
            </span>
          )}
          {tabs.length > 0 && (
            <ul className="tabmenu">
              {tabs.map((tab) => (
                <li key={tab.label} className={tab.selected ? "selected" : undefined}>
                  <Link href={tab.href} aria-current={tab.selected ? "page" : undefined}>
                    {tab.label}
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div id="header-bottom-right">
          <span className="readonly-badge">read-only</span> no login needed
        </div>
      </div>
    </header>
  );
}

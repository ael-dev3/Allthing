"use client";

import { useSearchParams } from "next/navigation";
import { use, useEffect } from "react";
import { channelKeyFromParam } from "@/lib/channels";
import { parsePage } from "@/lib/listing";
import { hashFromParam, usernameFromParam } from "@/lib/params";
import {
  loadChannelDirectory,
  loadChannelPage,
  loadFrontPage,
  loadPostPage,
  loadUserPage,
} from "@/lib/queries";
import { parseSort } from "@/lib/ranking";
import { parseCommentSort } from "@/lib/thread";
import { ChannelView, channelTitle } from "@/views/ChannelView";
import { ChannelsView } from "@/views/ChannelsView";
import { FrontPageView } from "@/views/FrontPageView";
import { NotFoundView } from "@/views/NotFoundView";
import { PostView, postTitle } from "@/views/PostView";
import { UserView, parseUserTab, userTitle } from "@/views/UserView";
import { browserSource, load } from "./load";

/*
 * Pages for the static build. Each reads its query string, loads data in the
 * browser (suspending until it arrives), and renders the same view the server
 * build renders.
 */

function useTitle(title: string | null) {
  useEffect(() => {
    if (title) document.title = title;
  }, [title]);
}

export function ClientFrontPage() {
  const params = useSearchParams();
  const { value, loadedAt } = use(load("front", () => loadFrontPage(browserSource())));
  return (
    <FrontPageView
      data={value}
      sort={parseSort(params.get("sort"))}
      page={parsePage(params.get("page") ?? undefined)}
      now={loadedAt}
      sourceName={browserSource().name}
    />
  );
}

export function ClientChannelPage() {
  const params = useSearchParams();
  const key = channelKeyFromParam(params.get("id") ?? "");
  const loaded = key ? use(load(`channel:${key}`, () => loadChannelPage(browserSource(), key))) : null;
  const data = loaded?.value ?? null;
  useTitle(data ? channelTitle(data.channel.id) : null);
  if (!loaded || !data) return <NotFoundView />;
  return (
    <ChannelView
      data={data}
      sort={parseSort(params.get("sort"))}
      page={parsePage(params.get("page") ?? undefined)}
      now={loaded.loadedAt}
    />
  );
}

export function ClientPostPage() {
  const params = useSearchParams();
  const hash = hashFromParam(params.get("hash"));
  const loaded = hash ? use(load(`post:${hash}`, () => loadPostPage(browserSource(), hash))) : null;
  const data = loaded?.value ?? null;
  useTitle(data ? postTitle(data.thread.cast) : null);
  if (!loaded || !data) return <NotFoundView />;
  return <PostView data={data} sort={parseCommentSort(params.get("sort"))} now={loaded.loadedAt} />;
}

export function ClientUserPage() {
  const params = useSearchParams();
  const username = usernameFromParam(params.get("name"));
  const loaded = username ? use(load(`user:${username}`, () => loadUserPage(browserSource(), username))) : null;
  const data = loaded?.value ?? null;
  useTitle(data ? userTitle(data.user.username) : null);
  if (!loaded || !data) return <NotFoundView />;
  return (
    <UserView
      data={data}
      tab={parseUserTab(params.get("tab"))}
      sort={parseSort(params.get("sort"), "new")}
      page={parsePage(params.get("page") ?? undefined)}
      now={loaded.loadedAt}
    />
  );
}

export function ClientChannelsPage() {
  const params = useSearchParams();
  const query = params.get("q")?.trim() ?? "";
  const { value } = use(load(`channels:${query}`, () => loadChannelDirectory(browserSource(), query)));
  return <ChannelsView query={query} channels={value} />;
}

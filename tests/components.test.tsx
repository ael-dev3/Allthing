import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { CommentTree } from "@/components/CommentTree";
import { PostList, PostRow } from "@/components/PostRow";
import { normalizeCast, normalizeThread, type RawCast } from "@/lib/datasource/normalize";
import type { Cast } from "@/lib/types";
import conversation from "./fixtures/hypersnap-conversation.json";
import trending from "./fixtures/hypersnap-feed-trending.json";

const casts: Cast[] = trending.casts.map((raw) => normalizeCast(raw as RawCast));
const textOf = (html: string) =>
  html
    .replace(/<[^>]+>/g, "")
    .replace(/&amp;/g, "&")
    .replace(/&#x27;|&#39;/g, "'")
    .replace(/\s+/g, " ")
    .trim();

describe("PostRow", () => {
  const swarm = casts[0];
  const now = new Date(swarm.timestamp).getTime() + 3 * 3_600_000;
  const html = renderToStaticMarkup(<PostRow cast={swarm} rank={1} now={now} />);

  it("renders rank, score, title and the old reddit meta line", () => {
    expect(html).toContain('<span class="rank">1</span>');
    expect(html).toContain('<span class="score"');
    expect(html).toContain(">552<"); // 184 likes + 2 x 184 recasts
    expect(html).toMatch(/<a class="title"[^>]* href="\/p\/0x09eb33c944884cf1bbe2713ddcc87056fdd627b2"/);
    expect(textOf(html)).toContain("submitted 3h ago by @swarmhood to /c/farcaster | 185 comments");
  });

  it("marks rows for keyboard navigation", () => {
    expect(html).toContain('data-nav=""');
    expect(html).toContain('data-nav-primary=""');
  });

  it("uses a resized image thumbnail or a placeholder", () => {
    const dwr = renderToStaticMarkup(<PostRow cast={casts[2]} now={now} />);
    expect(dwr).toMatch(/<img src="https:\/\/imagedelivery\.net\/[^"]+\/anim=false,fit=cover,f=auto,w=140,h=140"/);
    expect(dwr).toContain("[image]");
    const text = renderToStaticMarkup(<PostRow cast={casts[3]} now={now} />);
    expect(text).toContain("thumbnail placeholder self");
    expect(text).toContain("(self.farcaster)");
  });

  it("numbers ranks across pages", () => {
    const list = renderToStaticMarkup(<PostList casts={casts.slice(0, 2)} offset={25} now={now} />);
    expect(list).toContain('<span class="rank">26</span>');
    expect(list).toContain('<span class="rank">27</span>');
    expect(renderToStaticMarkup(<PostList casts={[]} now={now} />)).toContain("anything here");
  });
});

describe("CommentTree", () => {
  const tree = normalizeThread(conversation.conversation.cast as RawCast);
  const now = new Date(tree.cast.timestamp).getTime() + 86_400_000;
  const html = renderToStaticMarkup(<CommentTree nodes={tree.children} opFid={tree.cast.author.fid} now={now} />);

  it("renders every reply as a collapsible details element", () => {
    const comments = html.match(/<details open="" class="thing comment"/g) ?? [];
    expect(comments.length).toBe(14);
    expect(html).toContain('<span class="expand" aria-hidden="true"></span>');
  });

  it("nests children and flags the submitter", () => {
    expect(html).toContain('class="child"');
    expect(html).toMatch(/class="author submitter" href="\/u\/balajis\.eth"/);
    expect(html).toContain("(5 children)");
  });

  it("links spliced mentions to profiles", () => {
    expect(html).toContain('<a href="/u/dylanb.eth">@dylanb.eth</a>');
  });
});

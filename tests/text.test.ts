import { describe, expect, it } from "vitest";
import { castTitle, needsBody, paragraphs, spliceMentions, tokenize, truncateTitle } from "@/lib/text";

describe("spliceMentions", () => {
  it("inserts mentions at zero-width UTF-8 byte offsets", () => {
    // "🌴" is 4 bytes in UTF-8 but 2 UTF-16 code units, so char offsets would land wrong.
    const text = "Nothing held back! 🌴\n\n tipped  576 UCI.";
    const out = spliceMentions(
      text,
      [
        { username: "specktacular", fid: 1 },
        { username: "dylanb.eth", fid: 2 },
      ],
      [
        { start: 25, end: 25 },
        { start: 33, end: 33 },
      ],
    );
    expect(out).toBe("Nothing held back! 🌴\n\n@specktacular tipped @dylanb.eth 576 UCI.");
  });

  it("handles mentions given out of order and at the very end", () => {
    const out = spliceMentions(
      "Good Night ",
      [
        { username: "b", fid: 2 },
        { username: "a", fid: 1 },
      ],
      [
        { start: 11, end: 11 },
        { start: 0, end: 0 },
      ],
    );
    expect(out).toBe("@aGood Night @b");
  });

  it("leaves text alone when mentions are already inline (Neynar style)", () => {
    const text = "hi @dwr how are you";
    expect(spliceMentions(text, [{ username: "dwr", fid: 3 }], [{ start: 3, end: 7 }])).toBe(text);
  });

  it("leaves text alone when ranges and mentions disagree", () => {
    expect(spliceMentions("hi", [{ username: "x", fid: 1 }], [])).toBe("hi");
  });
});

describe("truncateTitle", () => {
  it("collapses whitespace", () => {
    expect(truncateTitle("hello\n\n  world")).toBe("hello world");
  });

  it("cuts long text at a word boundary near 120 characters", () => {
    const text = `${"word ".repeat(40)}end`;
    const title = truncateTitle(text);
    expect(title.endsWith("…")).toBe(true);
    expect(Array.from(title).length).toBeLessThanOrEqual(121);
    expect(title).not.toMatch(/ …$/);
  });

  it("never splits an emoji", () => {
    const title = truncateTitle("🎉".repeat(130));
    expect(title).toBe(`${"🎉".repeat(120)}…`);
  });
});

describe("castTitle", () => {
  it("uses placeholders for text-less casts", () => {
    expect(castTitle({ text: "", embeds: [{ kind: "image", url: "https://i.imgur.com/x.png" }] })).toBe("[image]");
    expect(castTitle({ text: " ", embeds: [{ kind: "cast", hash: "0xabc", fid: 1 }] })).toBe("[quote cast]");
    expect(
      castTitle({ text: "", embeds: [{ kind: "link", url: "https://www.example.com/a", imageUrl: null, title: null }] }),
    ).toBe("[link] example.com");
    expect(castTitle({ text: "", embeds: [] })).toBe("[empty cast]");
  });
});

describe("needsBody", () => {
  it("shows the body for multi-line or truncated text only", () => {
    expect(needsBody("short")).toBe(false);
    expect(needsBody("line one\nline two")).toBe(true);
    expect(needsBody("x ".repeat(100))).toBe(true);
    expect(needsBody("")).toBe(false);
  });
});

describe("tokenize", () => {
  it("finds URLs, known mentions and channels", () => {
    const tokens = tokenize("gm @dwr, see https://example.com/a?b=1. join /farcaster now", [
      { username: "dwr", fid: 3 },
    ]);
    expect(tokens).toEqual([
      { type: "text", text: "gm " },
      { type: "mention", text: "@dwr", username: "dwr" },
      { type: "text", text: ", see " },
      { type: "url", text: "https://example.com/a?b=1", href: "https://example.com/a?b=1" },
      { type: "text", text: ". join " },
      { type: "channel", text: "/farcaster", channel: "farcaster" },
      { type: "text", text: " now" },
    ]);
  });

  it("does not link @words that are not mentions, or paths inside words", () => {
    const tokens = tokenize("email me@example.com and/or @nobody");
    expect(tokens).toEqual([{ type: "text", text: "email me@example.com and/or @nobody" }]);
  });

  it("keeps balanced parentheses in URLs", () => {
    const [, url] = tokenize("(see https://en.wikipedia.org/wiki/Foo_(bar))");
    expect(url).toEqual({
      type: "url",
      text: "https://en.wikipedia.org/wiki/Foo_(bar)",
      href: "https://en.wikipedia.org/wiki/Foo_(bar)",
    });
  });

  it("links mentions with dots in the name", () => {
    const tokens = tokenize("thanks @balajis.eth.", [{ username: "balajis.eth", fid: 37 }]);
    expect(tokens).toEqual([
      { type: "text", text: "thanks " },
      { type: "mention", text: "@balajis.eth", username: "balajis.eth" },
      { type: "text", text: "." },
    ]);
  });
});

describe("paragraphs", () => {
  it("splits on blank lines and drops empty ones", () => {
    expect(paragraphs("a\nb\n\n\nc\n")).toEqual(["a\nb", "c"]);
  });
});

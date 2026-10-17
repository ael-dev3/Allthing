import Link from "next/link";
import { Fragment, type ReactNode } from "react";
import { channelHref } from "@/lib/channels";
import { userHref } from "@/lib/links";
import { paragraphs, tokenize } from "@/lib/text";
import type { Mention } from "@/lib/types";

function withLineBreaks(text: string): ReactNode {
  const lines = text.split("\n");
  return lines.map((line, i) => (
    <Fragment key={i}>
      {i > 0 && <br />}
      {line}
    </Fragment>
  ));
}

function renderTokens(text: string, mentions: Mention[]): ReactNode {
  return tokenize(text, mentions).map((token, i) => {
    switch (token.type) {
      case "text":
        return <Fragment key={i}>{withLineBreaks(token.text)}</Fragment>;
      case "url":
        return (
          <a key={i} href={token.href} rel="nofollow ugc noopener noreferrer">
            {token.text}
          </a>
        );
      case "mention":
        return (
          <Link key={i} href={userHref(token.username)}>
            {token.text}
          </Link>
        );
      case "channel":
        return (
          <Link key={i} href={channelHref(token.channel)}>
            {token.text}
          </Link>
        );
    }
  });
}

/** Cast text as paragraphs, with links, @mentions and /channels made clickable. */
export function CastText({ text, mentions = [] }: { text: string; mentions?: Mention[] }) {
  return (
    <>
      {paragraphs(text).map((paragraph, i) => (
        <p key={i}>{renderTokens(paragraph, mentions)}</p>
      ))}
    </>
  );
}

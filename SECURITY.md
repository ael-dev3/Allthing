<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Security

Allthing is a read-only client. It has no accounts, sessions, cookies, keys or write paths, and the deployed Worker accepts only `GET` and `HEAD`. The repository contains no secrets: upstream data sources are public and keyless, and deployment credentials live in Cloudflare Workers Builds.

## Reporting a vulnerability

Please report privately, using GitHub's **private vulnerability reporting** on this repository (Security → Report a vulnerability). If that isn't available, open a minimal issue asking for a private contact, without exploit details. Don't put tokens, keys or personal data in public issues.

Particularly relevant reports:

- script injection through cast text, embeds, profile fields or channel data;
- weaknesses in the Content Security Policy (`web/_headers`), or ways to frame or redirect the site;
- ways to make the Worker hammer an upstream, exceed its subrequest or CPU budget, or poison the edge cache for other readers;
- anything that would let Allthing write to Farcaster or act on a user's behalf.

## Design commitments

- Cast text is tokenized and rendered as text, never as HTML. Outbound links get `rel="noopener noreferrer nofollow ugc"`.
- Third-party video players load only after a click, and their iframes are limited by `frame-src`.
- Media is loaded straight from its host. The Worker never proxies media.
- The API sends no CORS headers, rate-limits per client and refuses other methods with 405.

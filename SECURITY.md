<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Security policy

Allthing is a read-only client. It has no accounts, sessions, cookies, keys or write paths, and the deployed Worker accepts only `GET` and `HEAD`. The repository contains no secrets: upstream data sources are public and keyless, and deployment credentials live in Cloudflare Workers Builds.

## Supported versions

Fixes land on `main`, which Cloudflare deploys to https://allthing.social, and ship in the next release. Older releases are not patched.

## Reporting a vulnerability

Please report privately, not in a public issue:

1. **Use GitHub's private vulnerability reporting.** On this repository's **Security** tab, choose **Report a vulnerability**. That opens a private security advisory that only you and the maintainer can see, where the fix can be discussed and the advisory published once it is deployed.
2. **If that option isn't shown**, open a minimal issue asking the maintainer, [@ael-dev3](https://github.com/ael-dev3), for a private channel, without any details of the problem.

Include what you can: the URL, file or endpoint involved, the steps to reproduce, and the impact you expect. Don't put tokens, keys or personal data in public issues. Please don't load-test or scan https://allthing.social, which would take its share of the Free plan from readers and load the public sources it reads; a local build (`npx wrangler dev`) runs the same code.

Particularly relevant:

- script injection through cast text, embeds, profile fields or channel data;
- weaknesses in the Content Security Policy (`web/_headers`), or ways to frame or redirect the site;
- ways to make the Worker hammer an upstream, exceed its subrequest or CPU budget, or poison the edge cache for other readers;
- anything that would let Allthing write to Farcaster or act on a user's behalf.

Problems in the services Allthing reads from (Hypersnap, farcaster.xyz, the public RPCs) or in Cloudflare's platform are best reported to their operators. Content posted on Farcaster, such as spam, isn't a vulnerability in Allthing.

## Design commitments

- Cast text is tokenized and rendered as text, never as HTML. Outbound links get `rel="noopener noreferrer nofollow ugc"`.
- Third-party video players load only after a click, and their iframes are limited by `frame-src`.
- Media is loaded straight from its host. The Worker never proxies media.
- The API sends no CORS headers, rate-limits per client and refuses other methods with 405.

<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Security policy

Allthing is a client for reading Farcaster. Readers can sign in with Farcaster by QR code ([docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#sign-in)), which only says who is reading: the Worker checks the signed message, then keeps a stateless session in an HMAC-signed, HttpOnly, Secure cookie. A signed-in reader can like, recast and post with an app key they approve in the Farcaster app ([docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#liking-and-recasting)): their browser makes and keeps it (WebCrypto Ed25519, not exportable), signs with it, and hands the messages to a Farcaster node; the Worker only endorses keys and never sees one. Casts, replies and quotes are signed and sent the same way, or, without a key, open in the Farcaster app, which posts them. Apart from the sign-in and endorsement endpoints, which take same-origin `POST`s, the deployed Worker accepts only `GET` and `HEAD`. The repository contains no secrets: upstream data sources are public and keyless, the session key is the Worker secret `SESSION_SECRET`, the custody key of Allthing's own Farcaster account (which endorses readers' keys) is the Worker secret `ALLTHING_CUSTODY_KEY`, KLIPY's app key for GIF search is the Worker secret `KLIPY_API_KEY` (handed to signed-in browsers, which KLIPY requires to call it themselves), and deployment credentials live in Cloudflare Workers Builds.

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
- ways to sign in as an account without its Farcaster app's approval, to forge, replay or steal a session cookie, or to sign someone in or out from another site;
- anything that would let Allthing, or anyone else, write to Farcaster as a reader without their pressing a button, or with a key they didn't approve, or read a reader's key out of their browser.

Problems in the services Allthing reads from (Hypersnap, farcaster.xyz, the public RPCs) or in Cloudflare's platform are best reported to their operators. Content posted on Farcaster, such as spam, isn't a vulnerability in Allthing.

## Design commitments

- Cast text is tokenized and rendered as text, never as HTML. Outbound links get `rel="noopener noreferrer nofollow ugc"`.
- Third-party video players load only after a click, and their iframes are limited by `frame-src`.
- Media is loaded straight from its host. The Worker never proxies media.
- The API sends no CORS headers, rate-limits per client and refuses other methods with 405.
- A reader's app key is made by their browser, unexportable, and stays there. The Worker's custody key only endorses keys, for signed-in sessions, for an hour; the Worker never signs as a reader or submits anything to Farcaster.
- Sign-in follows Farcaster's rules as `@farcaster/auth-client` checks them: a nonce bound to the browser that started it, the site's own domain, and a signer that OP Mainnet's registries confirm. The session cookie is HttpOnly, Secure and SameSite, and its key never leaves Cloudflare.

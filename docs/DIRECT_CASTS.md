<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Direct casts

How a Farcaster client other than the Farcaster app can read and send direct casts (DCs), as of 21 October 2026, and what Allthing can use. "Checked" means read in the docs or the code, or seen in the API's own headers on that day. "Inferred" means a reading of those, not something seen.

## Conclusion

Direct casts are not part of the Farcaster protocol. They are stored on the Farcaster app's servers, `api.farcaster.xyz`, which Network School has run since it bought Farcaster from Neynar on 15 October 2026. A client reaches them only through that app's API.

The one usable way in is the Direct Cast API (`/fc/conversation-list`, `/fc/conversation`, `/fc/message-list`, `/fc/message`, `/fc/group-list`), which is in beta. It takes the reader's own Farcaster API key (`wc_secret_…`), which they make at farcaster.xyz/~/developers/api-keys. herocast uses it this way.

Allthing's app key, the signer the reader approves for @allthingapp, doesn't help:

- The DC docs name only API keys.
- The app's own DC endpoints take a session token signed by the reader's custody key, which Allthing must never ask for.

The catch is CORS. `api.farcaster.xyz` lets any website call only the older send endpoint, `PUT /v2/ext-send-direct-cast`, which is marked for deprecation. The endpoints that read DCs answer cross-origin requests only from the app's own sites and localhost.

So a page on allthing.social that holds the reader's key can send a 1:1 DC itself, but it can't list conversations or read them. Reading needs one of two things:

- A relay on Allthing's Worker, where the key passes through Allthing's server.
- Farcaster's operator adding allthing.social to its CORS allowlist.

Neynar has no DC API. XMTP, which the Base app uses for chat, is a separate network that never shows Farcaster DCs.

## What exists

| | Paths | Auth | Limits |
|---|---|---|---|
| **Direct Cast API** (Notion, "Direct Cast API Reference [Public]", 22 Oct 2024, edited 4 Apr 2025: "limited beta… Please don't share this widely") | `GET /fc/conversation-list?category=default\|request\|archived`, `GET /fc/conversation`, `GET /fc/message-list` (newest first, `next.cursor`), `PUT /fc/message` (to a conversationId, groupId or recipientFid; up to 1,024 characters; `inReplyToMessageId`; an `idempotency-key` header), `GET /fc/group-list`, and more for groups | `Authorization: Bearer <API key>` | Sending: 5,000 a day, and 30 a minute per conversation. New conversations: 5,000 a day. Reads: "None applied at this time". A group is readable only by its admins. |
| **Programmable DCs v1** (Notion, 6 May 2024, edited 26 Jun 2025: "This API will soon be deprecated and replaced by new DC APIs") | `PUT /v2/ext-send-direct-cast` with `{recipientFid, message, idempotencyKey}` | `Authorization: Bearer <API key>` | 5 messages a minute to one recipient, 5,000 a day; 429 when exceeded. 403 when the recipient's DC settings refuse the message. |
| **The app's own** (undocumented; the farcaster.xyz web app's code) | `/v2/direct-cast-inbox`, `-conversation-list`, `-conversation-messages`, `-send` and more | A session token minted from a signature by the custody address (EIP-191, `generateToken`) | n/a |
| **Hand-off links** (docs.farcaster.xyz, "Direct Casts") | `https://farcaster.xyz/~/inbox/create/<fid>?text=<message>`, and the inbox at `https://farcaster.xyz/~/inbox` | none: the reader is signed in to the app | n/a |

**The API key.** A reader makes one on the "Developers: API Keys" page, farcaster.xyz/~/developers/api-keys. If it is hidden, it appears after Settings → Developer tools → "Enable developer tools", then Developers → API Keys. The key:

- takes a description and an expiry, one year ahead by default;
- has no scopes;
- is shown once;
- can be revoked on the same page, with up to about 50 per account.

Its documented powers are the DC endpoints above, acting as the reader.

**The protocol.** Snapchain v0.14.3 (15 Oct 2026) has no DC message type. Its `MessageType` list ends at `CHANNEL_MODERATE = 21`. FIP #99, "Direct Casts", has been an idea since June 2023. The docs' "plans to add direct casts to the protocol later this year" was written in July 2024.

## Options

| Approach | Read | Send | Auth | Needs | Risks |
|---|---|---|---|---|---|
| Protocol (Snapchain) | No | No | n/a | n/a | DCs aren't on it |
| Hand-off links to the Farcaster app | In the app | In the app | None | Nothing | The reader leaves Allthing |
| `PUT /v2/ext-send-direct-cast` from the browser | No | 1:1 text | Reader's API key | The reader's key, kept in their browser | Marked "soon deprecated" (2024); 403 when the recipient's settings refuse; the key has no scopes |
| `/fc/*` from the browser | 1:1, plus groups where the reader is admin | Yes | Reader's API key | Farcaster's operator to allowlist allthing.social for CORS | Blocked by CORS today; a beta API whose owner changed twice in 2026 |
| `/fc/*` through a relay on the Worker | 1:1, plus admin groups | Yes | Reader's API key, through the Worker | Allthing's owner to turn the relay on | The key leaves the reader's browser; a beta API |
| App Key token (`type: "app_key"`) on `/fc/*` | Unknown | Unknown | The key the reader approved for Allthing | One test by the owner, with his own account | Documented only for channel and block endpoints; probably refused; would still need the allowlist or the relay |
| The app's private `/v2/direct-cast-*` | All | All | A session token signed by the custody key | A seed phrase or wallet signature | Ruled out by Allthing's rules; the Terms bar "non-public areas" |
| Neynar | No | No | n/a | n/a | It has no DC API |
| XMTP | XMTP only | XMTP only | An XMTP key (a wallet to be findable) | A wallet | Not Farcaster's DCs |

## CORS, as `api.farcaster.xyz` answered on 21 October 2026

Preflights (`OPTIONS`) with `Origin: https://allthing.social`, and GETs without credentials:

- **`/v2/ext-send-direct-cast`** answers:
  - `access-control-allow-origin: *`
  - `access-control-allow-methods: GET, PUT, POST, DELETE, PATCH`
  - `access-control-allow-headers` listing `Authorization`, `Content-Type` and `Idempotency-Key`, among others
  - `access-control-expose-headers: x-ratelimit-limit, x-ratelimit-remaining, x-ratelimit-reset`
  - `access-control-max-age: 3600`, and no `allow-credentials`

  **A browser may send with it.** A path that doesn't exist gets no `access-control-allow-origin`, so the route is still there.
- **These endpoints:** `/fc/conversation-list`, `/fc/conversation`, `/fc/message`, `/fc/message-list`, `/fc/group`, `/fc/group-list`, `/v2/direct-cast-*` and `/v2/validate-api-key`.
  - Their preflight answers 200 with the same methods and headers, but **no `access-control-allow-origin`** for allthing.social.
  - The same is true for every other third-party origin tried: example.com, herocast.xyz, supercast.xyz, base.app, neynar.com, a `*.pages.dev` and a `*.github.io`.
  - Of the origins tried, the response echoes back only farcaster.xyz, www.farcaster.xyz, warpcast.com, wallet.farcaster.xyz, and `http://localhost:3000` and `http://localhost:8787`.
  - Without a key they answer 401 `{"errors":[{"message":"Authentication required"}]}`.

  **A browser on allthing.social can't read DCs.**
- A relay that never sees the key, tunnelling TLS through the Worker, isn't possible. Workers' TCP sockets can't reach Cloudflare's addresses, and `api.farcaster.xyz` is served through Cloudflare.

## Terms

Not legal advice: a reading of the documents, on 21 October 2026.

There are no API terms for these endpoints. The DC docs say to "treat this key as a secret", that "Abuse will result in access to the endpoint revoked", and that the beta is "limited… don't share this widely".

The Farcaster Terms of Use (NS0 PTE LTD, Network School; last updated 16 October 2026) ask users to:

- "keep secure and confidential your account password or any identification credentials we provide you";
- not "disclose your Account login credentials to anyone".

They forbid:

- "any robot, spider, or other automatic device, process, or means to access the Services";
- using "non-public areas";
- any "commercial purpose, including … competing with the Company's services".

Nothing allows or forbids a reader's using their own key in another client. A key that stays in the reader's own browser is the more defensible reading. Sending it through Allthing's server is closer to disclosing it, and is why the relay is off unless Allthing's owner turns it on.

## In Allthing

`/messages` reads and sends a signed-in reader's Direct Casts with the key they paste, which only their browser keeps ([ARCHITECTURE.md](ARCHITECTURE.md#direct-casts)). It reads through the Direct Cast API where that answers the browser; nowhere does today, so it says why, links to the Farcaster app's inbox, and sends with `ext-send-direct-cast`. `allthing_core::dc` holds:

- the API's shapes, read from fixtures shaped like the beta docs (`crates/allthing-core/tests/fixtures/dc-docs-*.json`), not captured responses;
- the checks on a pasted key (`wc_secret_` and letters, digits, `_` or `-`), a message (1,024 UTF-16 units, as the app's JavaScript counts) and a recipient's fid;
- the links that hand a message to the Farcaster app.

## Sources

All were read on 21 October 2026.

- Farcaster docs, "Direct Casts" (created 8 July 2024): https://docs.farcaster.xyz/reference/farcaster/direct-casts
- Farcaster docs, "Client API Reference", whose App Key authentication was added 7 October 2024: https://docs.farcaster.xyz/reference/farcaster/api
- "Programmable DCs v1 [Public]": https://farcasterhq.notion.site/Programmable-DCs-v1-Public-50d9d99e34ac4d10add55bd26a91804f
- "Direct Cast API Reference [Public]": https://farcasterhq.notion.site/Direct-Cast-API-Reference-Public-1276a6c0c1018089af2bda0d1697a2fd
- The farcaster.xyz web app's JavaScript (the API keys and developer tools pages, its API client), and its Terms of Use: https://farcaster.xyz/~/terms-of-use
- Snapchain v0.14.3, `proto/definitions/message.proto`: https://github.com/farcasterxyz/snapchain
- FIP #99, "Direct Casts" (20 June 2023): https://github.com/farcasterxyz/protocol/discussions/99
- Neynar, "Neynar is acquiring Farcaster" (21 January 2026): https://neynar.com/blog/neynar-is-acquiring-farcaster
- The Block, Network School acquires Farcaster (15 October 2026): https://www.theblock.co/news/ecosystems/2026-10-15-network-school-acquires-farcaster-from-neynar-418720
- Neynar's docs index: https://docs.neynar.com/llms.txt
- herocast's DMs (`src/common/constants/directCast.ts`, `app/api/dms/`): https://github.com/hero-org/herocast
- The Base app's XMTP chat (The Block, 16 July 2025): https://www.theblock.co/post/362713
- Cloudflare Workers' TCP sockets: https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/

<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# The NS vote

Profiles show an account's NS vote: how it answered when farcaster.xyz asked everyone to **stay** or **leave**, or that it did not vote. This page describes what the vote was, where each answer is recorded, and how Allthing reads it within the Workers request budget. The research was done on 2026-10-18 against Base mainnet and the farcaster.xyz client API.

## What was voted on

On 2026-10-15, Farcaster was sold by Neynar, its steward since 2025, to Balaji Srinivasan's **Network School**. The farcaster.xyz client then asked each account to accept a "Network School agreement" (the "social smart contract"):

- **Vote to stay**: "you're voting for the Network School Farcaster core team to set the rules of the reference client." The signed text reads: "I vote to stay. I accept the Network School agreement and want to keep participating on the farcaster.xyz client."
- **Vote to leave**: the account becomes read-only *in farcaster.xyz*. Keys, wallet and funds stay with the user, and other clients (like Allthing) are unaffected.
- **Not now** postpones the prompt for 24 hours. A voter can change their vote later.

It is an open-ended opt-in rather than a poll with a closing date. The contract was deployed on 2026-10-14, no end date has been published, and votes were still arriving on 2026-10-18.

## Where each answer lives

**Stay is onchain**, on Base:

- Contract: [`0x4ecfEd276c566AeB49Da1f3A888E8Dbb7263028D`](https://basescan.org/address/0x4ecfEd276c566AeB49Da1f3A888E8Dbb7263028D), "Network School Farcaster client" (symbol `VOTE`), an ERC-721.
- **Minting the opt-in**:
  - The user's farcaster.xyz app wallet signs an EIP-712 `OptIn(uint256 fid, address to, uint256 deadline, uint256 generation, string agreement)`.
  - Network School's relayer co-signs it and sends `mint(fid, to, deadline, generation, agreement, userSig, authorizerSig)` (selector `0x0a815f2c`), so users pay no gas.
  - The NFT's **token id is the FID**, and the contract emits `OptInMinted(uint256 indexed fid, address indexed to)` (topic0 `0xb51e0080…a1e315`).
- **Example**: transaction [`0x123ba408…b2db`](https://basescan.org/tx/0x123ba408b274b28449f1594bf283e578851faa0c5fa9c14e1bd4115e0b93b2db) minted FID **539854** (@0xael.eth), which voted to stay, to `0x76d0…8942`, that account's farcaster.xyz wallet.
- **Moderation**: Network School can **revoke** a stay vote. `revoke(uint256 fid, uint256 generation, uint256 deadline, bytes sig)` (selector `0xa0c720a9`) needs only Network School's authorizer signature, not the voter's. It sets `minted[fid]` to false, raises `mintGeneration(fid)` (selector `0xae36b5b8`) by one, burns the NFT and emits `OptInRevoked(uint256 indexed fid, address indexed owner)`.
  - **A revoke cancels the vote.** It puts the FID back where it was before voting; it records no "no" vote, and the contract has no function or event for one.
  - **The FID can vote again** with new signatures for the new generation. By 2026-10-18, 17 revokes had hit 16 FIDs, and 5 of those FIDs had voted again.
  - **The token can't be transferred** (`transferFrom` reverts with `TransferDisabled()`).
  - The contract's source isn't published. These behaviours were read from its bytecode (selectors matched by keccak, each function's checks, storage writes and events traced) and from every `OptInMinted` and `OptInRevoked` event since deployment.

**Leave is offchain only.** farcaster.xyz records it in its own backend (`POST /v2/opt-in/opt-out`), and nothing is written to any chain.

The wallet that holds the NFT is the farcaster.xyz app wallet, not the custody address and not necessarily the primary verified address. **Votes must be looked up by FID, never by address.**

## How Allthing reads it

`GET /api/v1/vote?fid=…` makes two requests in parallel and edge-caches the answer for an hour (`VOTE_CACHE`):

1. **Base**: one batched JSON-RPC request with two `eth_call`s to the contract:
   - `minted(uint256 fid)` (selector `0x7dc0bf3f`) returns 1 while the FID holds the stay NFT;
   - `mintGeneration(uint256 fid)` (selector `0xae36b5b8`) returns how many times its vote was revoked.
2. **farcaster.xyz**: `GET https://client.farcaster.xyz/v2/user-by-fid?fid=…`. This public endpoint needs no authentication and returns `result.user.viewerContext.optInStatus`, which is one of `opted_in`, `opted_out`, `moderated_out` or `pending`.

That is two subrequests per profile view, at most once an hour per data center.

The profile shows only **Voted yes** (stay), **Voted no** (leave) or **Didn't vote** (`allthing_core::vote::VotePayload::summary`). The tooltip names the source:

| Base | farcaster.xyz | Shown | Source named in the tooltip |
|---|---|---|---|
| minted = 1 | any | **Voted yes** | Base |
| minted = 0 (revoked or never minted) | `opted_out` | **Voted no** | farcaster.xyz (leave is not onchain) |
| minted = 0, generation > 0 (revoked) | anything else, or unreadable | **Didn't vote** | Base: the revoke cancelled the vote |
| minted = 0, generation = 0 | `pending` or `moderated_out` | **Didn't vote** | Base and farcaster.xyz |
| minted = 0, generation = 0 | `opted_in` | **Didn't vote** | Base, noting that farcaster.xyz says opted in |
| minted = 0, generation = 0 | unreadable | *(not shown)* | could be a leave vote |
| unreadable | `opted_in` / `opted_out` / `pending` or `moderated_out` | **Voted yes** / **Voted no** / **Didn't vote** | farcaster.xyz |
| unreadable | unreadable | *(not shown)* | |

Base decides wherever it can. The farcaster.xyz API is the operator's own claim, used for what Base can't know (leaving) and when Base can't be read. The two sources do sometimes disagree; for example, FID 191213 is `opted_in` at farcaster.xyz but has never been minted. While the vote loads, the profile shows an empty pill; when it can't be settled, the row is left out.

Checked on 2026-10-18:

| FID | Account | Base | farcaster.xyz | Shown |
|---|---|---|---|---|
| 539854 | @0xael.eth | minted | `opted_in` | Voted yes |
| 3 | @dwr | minted | `opted_in` | Voted yes |
| 5650 | @vitalik.eth | not minted | `pending` | Didn't vote |

## Alternatives considered

- **Indexing every `OptInMinted` and `OptInRevoked` log** into a map of all voters. On 2026-10-18 there were about 18,700 stay votes and a handful of revocations. The map is about 22 KB delta-encoded, but building it takes more than 140 `eth_getLogs` calls (public RPCs allow 200–1,000 blocks per call) and keeping it current needs a scheduled job. That would break the 45-subrequest budget per request. Per-FID calls are cheaper for a client that shows one profile at a time.
- **A Multicall3 batch** (`aggregate3`) of `minted` for many FIDs in one subrequest would let feeds show vote badges. It isn't used yet; profiles are the only place the vote appears.

## Sources

- The transaction and contract on Base, read directly through `eth_getTransactionByHash`, `eth_getTransactionReceipt` and `eth_call` on `mainnet.base.org` and `base-rpc.publicnode.com`. The selectors and event signatures were confirmed by keccak.
- The farcaster.xyz web app's vote flow (its client bundle and the `/v2/opt-in/*` endpoints) and `GET /v2/user-by-fid`.
- The vote page: https://farcaster.xyz/~/vote

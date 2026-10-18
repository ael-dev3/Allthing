<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Neynar user score: where Allthing reads it

The small number beside a name (0.00–1.00) is the **Neynar user score**, Neynar's estimate of how likely an account is a quality, non-spam account. This page records where Allthing reads it from and why. The sources were re-checked on 2026-10-18.

## Decision

Allthing reads the score **onchain, from Neynar's score contract on Base**:

- one `eth_call` of `getScores(uint256[])` per page, covering every author on it;
- through keyless public RPCs (`mainnet.base.org`, falling back to `base-rpc.publicnode.com`);
- memoized for 12 hours per Worker isolate, and edge-cached inside each page's payload.

Code: `crates/allthing-data/src/chain.rs` (`neynar_scores`, `encode_get_scores`, `decode_scores`) and `crates/allthing-data/src/annotate.rs` (`scores`).

## The sources compared

| Source | Keyless | Fresh | Cost per page | Verdict |
|---|---|---|---|---|
| **Score contract on Base** (`0xd3C43A38D1D3E47E9c420a733e439B03FAAdebA8`, an upgradeable proxy) | yes | Neynar publishes weekly | 1 subrequest for up to 150 FIDs | **used** |
| Neynar API (`experimental.neynar_user_score` on user objects) | no: needs an API key | same model | 0 extra, with a key | not used: Allthing holds no keys |
| Hypersnap's Neynar-shaped API | yes | n/a | 0 | **has no score**: `user/bulk`, `user/search` and feed authors carry neither `score` nor `experimental.neynar_user_score`, even with `x-neynar-experimental: true` |

Notes:
- **What Neynar documents**:
  - the score is published as `experimental.neynar_user_score`, and updated at least weekly (the model was retrained in May 2025);
  - the Base contract is the onchain copy and exposes `getScore(uint256)` (`0x0e1af57b`), `getScores(uint256[])` (`0x23d36428`) and address variants;
  - values are `uint24`, where 1,000,000 means 1.0;
  - there is a Base Sepolia test deployment and no newer mainnet contract.
- **Zero means "no score"**, not a score of zero; Allthing hides the badge.
- `normalize.rs` still reads `experimental.neynar_user_score` (or `score`) when a user object carries one. If Hypersnap starts returning it, Allthing uses it and saves the RPC call.

## Known FIDs

These values, read on Base at block `0x324ded7` (identical on both RPCs, and `getScore` agrees with `getScores`), are a unit test: `known_fids_score_as_published` in `crates/allthing-data/src/chain.rs`.

| FID | Account | Score |
|---|---|---|
| 3 | dwr | 1.00 |
| 2 | v | 1.00 |
| 539854 | 0xael.eth | 0.74 |
| 5650 | vitalik.eth | 0.96 |
| 194 | rish | 1.00 |

The end-to-end suite also checks that a live profile shows a score in `0.00`–`1.00` format.

## Sources

- https://docs.neynar.com/docs/neynar-user-quality-score
- https://docs.neynar.com/docs/address-user-score-contract
- https://neynar.notion.site/Experimental-Features-1d2655195a8b80eb98b4d4ae7b76ae4a
- https://neynar.com/blog/retraining-neynar-user-score-algorithm

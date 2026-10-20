<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Wallet activity by chain

The wallet chart on a profile (and on its hover card) shows where an account's verified wallets have been active, as a share of all the activity counted. This page says where each number comes from, how it is computed, and how it was checked.

## Sources

- **Verified addresses**: the account's verified Ethereum and Solana addresses, from Hypersnap's user record (`user/bulk`). Up to five EVM addresses are counted per account, and up to two Solana addresses.
- **EVM chains** (Base, Ethereum, Optimism, Arbitrum, Polygon, Zora, Celo, Unichain, Monad): each chain's public JSON-RPC endpoints, as listed in `crates/allthing-core/src/wallet.rs`. One batched `eth_getTransactionCount` call per chain, at `latest`.
- **Solana**: the signatures involving each verified address, read from a keyed RPC endpoint when the Worker has one (the `SOLANA_RPC_URL` secret, see [DEPLOY.md](DEPLOY.md#secrets)). It pages through the address's history, up to 5,000 signatures, and the count shows "5,000+" beyond that. Without the secret, Allthing reads one `getSignaturesForAddress` call (up to 1,000) from Solana's public RPC, which refuses Cloudflare Workers, so Solana then shows as unread and the chart says it couldn't be read just now.

## Method

- **EVM**: the count is the account nonce, which is the number of transactions each address has *sent*. Incoming transfers are not counted. It is exact, not an estimate.
- **Solana**: there is no nonce. The count is the number of signatures involving the address, which includes transfers in. A count at the cap shows with a plus: "5,000+" with a keyed endpoint (five pages of 1,000 signatures), "1,000+" with the public one (one page).
- **Shares**: a chain under 1% of the total (count × 100 < total) is not shown. The shares of the shown chains are divided by their own total, and rounded with the largest-remainder method, so they sum to exactly 100 and the spare points go only to shown chains. The browser works them out from the counts, so they follow these rules whatever the age of the payload. Example: 0xael.eth on 18 October 2026, where Optimism's 95 of 33,090 (0.29%) is hidden and Base shows 99%, Ethereum 1%.
- **Display**: the chart and its list show only the shown chains. One chain at 100% is the only row. The profile says what the shares are of ("Share of the verified wallets' transactions, by chain."), and "?" beside "Wallet activity", on the profile and its card, links to this page. A card names its two largest chains, then how many more there are and their share together ("+4 35%"), so the numbers it shows add up to 100. Its bar, and the bar's label, have every chain.
- **Unread chains**: if an EVM chain can't be read just now (rate limited, unreachable), it keeps its last good count from the previous copy of the account's wallet activity, when the account's verified addresses haven't changed, and the chart says that some counts are from an earlier read. A count only grows, so this is closer to the truth than leaving the chain out, which scaled the other chains up to its share (on 19 October 2026, 0xael.eth's chart showed Optimism 58% and Monad 41% while Base and Ethereum were unread). A chain with no earlier count is left out of the shares, the chart says some chains weren't read, and the copy is read again within minutes instead of hours. It is served from the background for ten minutes at most: after that, a reader waits for a new read, and gets this copy only if that read fails. When the account has verified Solana addresses and Solana can't be read, the chart says that Solana couldn't be read, and the payload's `sources` says so too.

## Cross-checks (18 October 2026)

Block explorers can't give the sent count directly, so the check is nonce-based: the nonce on an account's latest outgoing transaction, plus one, must equal the RPC's `latest` nonce. Basescan was used because Etherscan blocks automated requests.

| Address | Chain | RPC nonce | Newest outgoing nonce on Basescan | Result |
|---|---|---|---|---|
| 0x76d0…8942 (0xael.eth, fid 539854) | Base | 6,663 | 6,662 | matches |
| 0x5d13…c5ea (0xael.eth) | Base | 25,857 | 25,856 | matches |
| 0xd8da…6045 (vitalik.eth, fid 5650) | Base | 70 | 69 | matches |
| 0x8491…8bf1 (jessepollak, fid 99) | Base | 6,238 | 6,236 in the sample; 6,237 not in the 12 hashes sampled | consistent |

**Not reconciled**: Basescan's headline count for 0x76d0…8942 reads 3,441 transactions, which is below its nonce of 6,663. A nonce counts sent transactions, so the headline should not be lower. The explorer's figure is probably computed differently (for example, it may exclude some transaction types). The nonce is the stronger evidence, but I have not resolved the mismatch.

**Not checked**: Ethereum mainnet and the other EVM chains (Etherscan blocked the request), and Solana (no explorer reachable from this environment).

## Cross-check (20 October 2026)

0xael.eth's two verified EVM addresses, read directly from each chain's public RPC (`eth_getTransactionCount`, `latest`) and summed, against what `/api/v1/wallets?fid=539854` served:

| Chain | 0x76d0…8942 | 0x5d13…c5ea | Sum (RPC) | Allthing | Result |
|---|---|---|---|---|---|
| Base | 6,668 | 25,857 | 32,525 | 32,525 | matches |
| Ethereum | 193 | 195 | 388 | 388 | matches |
| Optimism | 36 | 59 | 95 | 95 | matches |
| Monad | 58 | 8 | 66 | 66 | matches |
| Arbitrum | 7 | 12 | 19 | 19 | matches |
| Celo | 2 | 0 | 2 | 2 | matches |
| Polygon, Zora, Unichain | 0 | 0 | 0 | 0 | matches |

The shares the profile and its card show, Base 99% and Ethereum 1%, follow from these: the four chains under 1% (0.55% together) are left out, and Base's 32,525 is 98.8% of the 32,913 shown. Solana wasn't counted: production has no keyed Solana endpoint, and the public one refuses the Worker (403). This checks Allthing's reads and sums against the chains; the nonce method itself was checked against Basescan on 18 October.

## Known errors and fixes

- A rate-limited chain was dropped from the shares without a word, so the chart looked complete. It now says that some chains weren't read.
- Chains under 1% showed as "<1%" rows. They are now hidden, so the chart shows only chains with a whole percent.
- For up to a day after that change, the edge kept serving wallet payloads it had cached with the old shares, so a chain under 1% could still show as 1% (0xael.eth's Optimism, 0.29%, on 18 October 2026). The browser now works out the shares from the counts itself.
- A copy cached before unread chains kept their last good count, with Base unread, was still served a day later (20 October 2026): the first reader of 0xael.eth's profile saw Optimism 58% and Monad 41%, while the copy refreshed behind it. A copy missing a chain is now served from the background for ten minutes at most.

<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Wallet activity by chain

The wallet chart on a profile (and on its hover card) shows where an account's verified wallets have been active, as a share of all the activity counted. This page says where each number comes from, how it is computed, and how it was checked.

## Sources

- **Verified addresses**: the account's verified Ethereum and Solana addresses, from Hypersnap's user record (`user/bulk`). Up to five EVM addresses are counted per account, and up to two Solana addresses.
- **EVM chains** (Base, Ethereum, Optimism, Arbitrum, Polygon, Zora, Celo, Unichain, Monad): each chain's public JSON-RPC endpoints, as listed in `crates/allthing-core/src/wallet.rs`. One batched `eth_getTransactionCount` call per chain, at `latest`.
- **Solana**: one `getSignaturesForAddress` call per address (up to 1,000 signatures) to Solana's public RPC. It refuses browsers and, from our tests, Cloudflare Workers' addresses, so Solana is usually "not measured" on the live site.

## Method

- **EVM**: the count is the account nonce, which is the number of transactions each address has *sent*. Incoming transfers are not counted. It is exact, not an estimate.
- **Solana**: there is no nonce. The count is the number of signatures involving the address, which includes transfers in. A count at the 1,000 cap shows as "1,000+".
- **Shares**: a chain under 1% of the total (count × 100 < total) is not shown. The shares of the shown chains are divided by their own total, and rounded with the largest-remainder method, so they sum to exactly 100 and the spare points go only to shown chains. Example: 0xael.eth on 18 October 2026, where Optimism's 95 of 33,090 (0.29%) is hidden and Base shows 99%, Ethereum 1%.
- **Display**: the chart and its list show only the shown chains. One chain at 100% is the only row.
- **Unread chains**: if an EVM chain can't be read just now (rate limited, unreachable), it is left out of the shares, and the chart says that some chains weren't read. Solana being unmeasured is not shown as an error.

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

## Known errors and fixes

- A rate-limited chain was dropped from the shares without a word, so the chart looked complete. It now says that some chains weren't read.
- Chains under 1% showed as "<1%" rows. They are now hidden, so the chart shows only chains with a whole percent.

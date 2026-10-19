// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wallet activity by chain, measured from public RPCs for a user's verified
//! addresses.
//!
//! EVM chains report how many transactions each address has *sent* (its
//! account nonce): exact, one batched call per chain. Solana has no nonce:
//! it counts the transactions involving each address (its signatures), up to
//! a cap, from the public RPC. That RPC refuses browsers, so the static copy
//! reports Solana as not measured.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChainKind {
    Evm,
    Solana,
}

#[derive(Clone, Copy, Debug)]
pub struct ChainInfo {
    /// Stable slug used in payloads and CSS ("base").
    pub slug: &'static str,
    pub name: &'static str,
    pub kind: ChainKind,
    /// Keyless public JSON-RPC endpoints, tried in order.
    pub rpcs: &'static [&'static str],
}

/// Chains measured, roughly in order of Farcaster wallet usage.
pub const CHAINS: &[ChainInfo] = &[
    ChainInfo {
        slug: "base",
        name: "Base",
        kind: ChainKind::Evm,
        rpcs: &["https://mainnet.base.org", "https://base-rpc.publicnode.com"],
    },
    ChainInfo {
        slug: "ethereum",
        name: "Ethereum",
        kind: ChainKind::Evm,
        rpcs: &["https://ethereum-rpc.publicnode.com"],
    },
    ChainInfo {
        slug: "optimism",
        name: "Optimism",
        kind: ChainKind::Evm,
        rpcs: &["https://mainnet.optimism.io", "https://optimism-rpc.publicnode.com"],
    },
    ChainInfo {
        slug: "arbitrum",
        name: "Arbitrum",
        kind: ChainKind::Evm,
        rpcs: &["https://arb1.arbitrum.io/rpc", "https://arbitrum-one-rpc.publicnode.com"],
    },
    ChainInfo {
        slug: "polygon",
        name: "Polygon",
        kind: ChainKind::Evm,
        rpcs: &["https://polygon-bor-rpc.publicnode.com"],
    },
    ChainInfo { slug: "zora", name: "Zora", kind: ChainKind::Evm, rpcs: &["https://rpc.zora.energy"] },
    ChainInfo {
        slug: "celo",
        name: "Celo",
        kind: ChainKind::Evm,
        rpcs: &["https://forno.celo.org", "https://celo-rpc.publicnode.com"],
    },
    ChainInfo {
        slug: "unichain",
        name: "Unichain",
        kind: ChainKind::Evm,
        rpcs: &["https://mainnet.unichain.org", "https://unichain-rpc.publicnode.com"],
    },
    ChainInfo { slug: "monad", name: "Monad", kind: ChainKind::Evm, rpcs: &["https://rpc.monad.xyz"] },
    // Solana's public RPC: it answers server-side requests, and refuses
    // browsers (it rejects any `Origin`), so the static copy can't measure it.
    ChainInfo {
        slug: "solana",
        name: "Solana",
        kind: ChainKind::Solana,
        rpcs: &["https://api.mainnet-beta.solana.com"],
    },
];

pub fn chain(slug: &str) -> Option<&'static ChainInfo> {
    CHAINS.iter().find(|c| c.slug == slug)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainActivity {
    pub chain: String,
    pub name: String,
    pub kind: ChainKind,
    /// Transactions counted across all of the user's addresses on this chain.
    pub count: u64,
    /// The count hit a cap, so the real number is at least this.
    #[serde(default)]
    pub capped: bool,
    /// Share of all counted transactions, in whole percent (sums to 100).
    #[serde(default)]
    pub percent: u32,
    /// The chain could not be measured; `count` is meaningless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The chain couldn't be read this time, so `count` is its last good
    /// count, from an earlier read of the same addresses.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub carried: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletAddress {
    pub address: String,
    pub kind: ChainKind,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletActivity {
    pub fid: u64,
    pub addresses: Vec<WalletAddress>,
    /// Measured chains, most active first; failed chains last.
    pub chains: Vec<ChainActivity>,
    pub total: u64,
    /// Human-readable description of each data source used.
    pub sources: Vec<String>,
    /// Unix milliseconds.
    pub fetched_at: i64,
}

/// Give each chain that couldn't be read now (`error`) its last good count
/// from `previous`, an earlier read of the same `addresses`: a count only
/// grows, so an older one is closer to the truth than leaving the chain out,
/// which would scale the other chains up to its share. Nothing is carried
/// when the account's addresses changed.
pub fn carry_unread(chains: &mut [ChainActivity], addresses: &[WalletAddress], previous: &WalletActivity) {
    fn sorted(addresses: &[WalletAddress]) -> Vec<&str> {
        let mut list: Vec<&str> = addresses.iter().map(|w| w.address.as_str()).collect();
        list.sort_unstable();
        list
    }
    if sorted(addresses) != sorted(&previous.addresses) {
        return;
    }
    for chain in chains.iter_mut().filter(|c| c.error.is_some()) {
        let Some(last) = previous.chains.iter().find(|p| p.chain == chain.chain && p.error.is_none()) else { continue };
        chain.count = last.count;
        chain.capped = last.capped;
        chain.error = None;
        chain.carried = true;
    }
}

/// Whether an EVM chain is still unread after [`carry_unread`]: the payload
/// is then refreshed soon, not hours later. Solana is left out: an unread
/// Solana usually means no keyed endpoint is configured, which a re-read
/// doesn't change.
pub fn evm_unread(chains: &[ChainActivity]) -> bool {
    chains.iter().any(|c| c.kind == ChainKind::Evm && c.error.is_some())
}

/// Fill in `percent` with the largest-remainder method, and order chains by
/// activity (failed chains last). A chain under 1% of the total is not shown
/// (percent 0), and the spare points go only to shown chains, so the shown
/// shares sum to exactly 100.
pub fn apportion(chains: &mut [ChainActivity]) -> u64 {
    let total: u64 = chains.iter().filter(|c| c.error.is_none()).map(|c| c.count).sum();
    let shown = |c: &ChainActivity| c.error.is_none() && c.count > 0 && c.count * 100 >= total;
    let shown_total: u64 = chains.iter().filter(|c| shown(c)).map(|c| c.count).sum();
    let mut remainders: Vec<(usize, u64)> = Vec::new();
    let mut assigned = 0u64;
    for (i, c) in chains.iter_mut().enumerate() {
        c.percent = 0;
        if !shown(c) {
            continue;
        }
        let scaled = c.count * 100;
        let share = scaled / shown_total;
        c.percent = share as u32;
        assigned += share;
        remainders.push((i, scaled % shown_total));
    }
    remainders.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (i, _) in remainders.into_iter().take((100 - assigned) as usize) {
        chains[i].percent += 1;
    }
    chains.sort_by(|a, b| {
        a.error.is_some().cmp(&b.error.is_some()).then(b.count.cmp(&a.count)).then(a.chain.cmp(&b.chain))
    });
    total
}

#[cfg(test)]
mod apportion_tests {
    use super::*;

    fn chain(name: &str, count: u64) -> ChainActivity {
        ChainActivity {
            chain: name.into(),
            name: name.into(),
            kind: ChainKind::Evm,
            count,
            capped: false,
            percent: 0,
            error: None,
            carried: false,
        }
    }

    fn unread(name: &str) -> ChainActivity {
        ChainActivity { error: Some("upstream returned HTTP 429".into()), ..chain(name, 0) }
    }

    fn address(a: &str) -> WalletAddress {
        WalletAddress { address: a.into(), kind: ChainKind::Evm, primary: false }
    }

    fn previous(addresses: &[&str], chains: Vec<ChainActivity>) -> WalletActivity {
        WalletActivity {
            fid: 539_854,
            addresses: addresses.iter().map(|a| address(a)).collect(),
            chains,
            total: 0,
            sources: vec![],
            fetched_at: 0,
        }
    }

    /// 0xael.eth on 19 Oct 2026: Base and Ethereum went unread, and the
    /// chart showed Optimism 58%, Monad 41%. Carried, Base leads again.
    #[test]
    fn an_unread_chain_keeps_its_last_good_count_and_the_shares_stay_true() {
        let last =
            previous(&["0xa", "0xb"], vec![chain("base", 33_000), chain("ethereum", 300), chain("optimism", 95)]);
        let mut now = vec![unread("base"), unread("ethereum"), chain("optimism", 96), chain("monad", 60)];
        carry_unread(&mut now, &[address("0xb"), address("0xa")], &last);
        assert!(!evm_unread(&now));
        let base = now.iter().find(|c| c.chain == "base").unwrap();
        assert_eq!((base.count, base.carried, base.error.is_none()), (33_000, true, true));
        apportion(&mut now);
        assert_eq!(now[0].chain, "base");
        assert!(now[0].percent >= 98, "{}", now[0].percent);
        assert!(!now.iter().find(|c| c.chain == "optimism").unwrap().carried);
    }

    #[test]
    fn nothing_is_carried_when_the_addresses_changed_or_the_chain_was_never_read() {
        let last = previous(&["0xa"], vec![chain("base", 33_000), unread("celo")]);
        let mut changed = vec![unread("base")];
        carry_unread(&mut changed, &[address("0xa"), address("0xc")], &last);
        assert!(changed[0].error.is_some() && !changed[0].carried);
        assert!(evm_unread(&changed));
        let mut never = vec![unread("celo")];
        carry_unread(&mut never, &[address("0xa")], &last);
        assert!(never[0].error.is_some());
    }

    /// 0xael.eth (fid 539854) on 18 Oct 2026: Optimism's 95 of 33,090 is 0.29%,
    /// so it is not shown, and the rest sum to 100.
    #[test]
    fn a_chain_under_one_percent_is_not_shown_and_the_rest_sum_to_100() {
        let mut chains = vec![
            chain("base", 32520),
            chain("ethereum", 388),
            chain("optimism", 95),
            chain("monad", 66),
            chain("arbitrum", 19),
            chain("celo", 2),
        ];
        assert_eq!(apportion(&mut chains), 33090);
        let pct = |name: &str| chains.iter().find(|c| c.chain == name).unwrap().percent;
        assert_eq!((pct("optimism"), pct("monad"), pct("arbitrum"), pct("celo")), (0, 0, 0, 0));
        assert_eq!((pct("base"), pct("ethereum")), (99, 1));
        assert_eq!(chains.iter().map(|c| c.percent).sum::<u32>(), 100);
    }

    #[test]
    fn shares_of_shown_chains_sum_to_exactly_100() {
        let mut chains = vec![chain("a", 1), chain("b", 1), chain("c", 1)];
        assert_eq!(apportion(&mut chains), 3);
        assert_eq!(chains.iter().map(|c| c.percent).collect::<Vec<_>>(), vec![34, 33, 33]);

        let mut chains = vec![chain("base", 800), chain("eth", 7), chain("zero", 0)];
        apportion(&mut chains);
        assert_eq!(chains.iter().find(|c| c.chain == "base").unwrap().percent, 100);
        assert_eq!(chains.iter().find(|c| c.chain == "eth").unwrap().percent, 0);
        assert_eq!(chains.iter().map(|c| c.percent).sum::<u32>(), 100);
    }

    #[test]
    fn no_activity_means_no_shares() {
        let mut chains = vec![chain("a", 0)];
        assert_eq!(apportion(&mut chains), 0);
        assert_eq!(chains[0].percent, 0);
    }
}

/// The user's verified addresses, primary first, deduplicated.
pub fn addresses(
    eth: &[String],
    sol: &[String],
    primary_eth: Option<&str>,
    primary_sol: Option<&str>,
) -> Vec<WalletAddress> {
    let mut out: Vec<WalletAddress> = Vec::new();
    let mut push = |address: &str, kind: ChainKind, primary: bool| {
        let address = if kind == ChainKind::Evm { address.to_ascii_lowercase() } else { address.to_owned() };
        if !out.iter().any(|a| a.address == address) {
            out.push(WalletAddress { address, kind, primary });
        }
    };
    if let Some(p) = primary_eth {
        push(p, ChainKind::Evm, true);
    }
    if let Some(p) = primary_sol {
        push(p, ChainKind::Solana, true);
    }
    for a in eth {
        push(a, ChainKind::Evm, false);
    }
    for a in sol {
        push(a, ChainKind::Solana, false);
    }
    out
}

/// Parse a JSON-RPC quantity ("0x301") as an integer.
pub fn parse_quantity(value: &str) -> Option<u64> {
    u64::from_str_radix(value.strip_prefix("0x")?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_addresses_primary_first() {
        let out = addresses(&["0xAA".into(), "0xbb".into()], &["Sol1".into()], Some("0xbb"), None);
        assert_eq!(out.iter().map(|a| a.address.as_str()).collect::<Vec<_>>(), ["0xbb", "0xaa", "Sol1"]);
        assert!(out[0].primary && !out[1].primary);
    }

    #[test]
    fn quantities() {
        assert_eq!(parse_quantity("0x301"), Some(769));
        assert_eq!(parse_quantity("0x0"), Some(0));
        assert_eq!(parse_quantity("301"), None);
        assert!(chain("base").is_some() && chain("nope").is_none());
    }
}

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

/// Fill in `percent` with the largest-remainder method so shares sum to 100,
/// and order chains by activity (failed chains last).
pub fn apportion(chains: &mut [ChainActivity]) -> u64 {
    let total: u64 = chains.iter().filter(|c| c.error.is_none()).map(|c| c.count).sum();
    let mut remainders: Vec<(usize, u64)> = Vec::new();
    let mut assigned = 0u64;
    for (i, c) in chains.iter_mut().enumerate() {
        c.percent = 0;
        if c.error.is_some() {
            continue;
        }
        let scaled = c.count * 100;
        if let (Some(share), Some(remainder)) = (scaled.checked_div(total), scaled.checked_rem(total)) {
            c.percent = share as u32;
            assigned += share;
            remainders.push((i, remainder));
        }
    }
    // Hand the points lost to rounding down to the largest remainders.
    if !remainders.is_empty() {
        remainders.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (i, _) in remainders.into_iter().take((100 - assigned) as usize) {
            chains[i].percent += 1;
        }
    }
    chains.sort_by(|a, b| {
        a.error.is_some().cmp(&b.error.is_some()).then(b.count.cmp(&a.count)).then(a.chain.cmp(&b.chain))
    });
    total
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

    fn activity(chain: &str, count: u64) -> ChainActivity {
        ChainActivity {
            chain: chain.into(),
            name: chain.into(),
            kind: ChainKind::Evm,
            count,
            capped: false,
            percent: 0,
            error: None,
        }
    }

    #[test]
    fn apportions_to_exactly_100() {
        let mut chains = vec![activity("a", 1), activity("b", 1), activity("c", 1)];
        assert_eq!(apportion(&mut chains), 3);
        assert_eq!(chains.iter().map(|c| c.percent).sum::<u32>(), 100);
        assert_eq!(chains.iter().map(|c| c.percent).collect::<Vec<_>>(), vec![34, 33, 33]);

        let mut chains = vec![activity("eth", 7), activity("base", 800), activity("op", 8), activity("zero", 0)];
        let mut failed = activity("sol", 0);
        failed.error = Some("timeout".into());
        chains.push(failed);
        assert_eq!(apportion(&mut chains), 815);
        assert_eq!(chains[0].chain, "base");
        assert_eq!(chains[0].percent, 98);
        assert_eq!(chains.iter().map(|c| c.percent).sum::<u32>(), 100);
        assert_eq!(chains.last().unwrap().chain, "sol");
    }

    #[test]
    fn no_activity_means_no_shares() {
        let mut chains = vec![activity("a", 0)];
        assert_eq!(apportion(&mut chains), 0);
        assert_eq!(chains[0].percent, 0);
    }

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

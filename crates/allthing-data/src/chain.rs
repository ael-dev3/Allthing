// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keyless public JSON-RPC reads: Neynar user scores (published onchain on
//! Base) and EVM transaction counts.

use std::collections::HashMap;

use allthing_core::vote::{MINTED_SELECTOR, OnchainVote, REVOKED_SELECTOR, VOTE_CONTRACT, encode_fid_call};
use allthing_core::wallet::{ChainInfo, parse_quantity};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Invocation;
use crate::upstream::{Upstream, UpstreamError, rpc};

/// Neynar's user-score contract on Base (an upgradeable proxy). Scores are
/// published weekly, scaled by 1e6; zero means "no score".
pub const NEYNAR_SCORES_CONTRACT: &str = "0xd3C43A38D1D3E47E9c420a733e439B03FAAdebA8";
const BASE_RPCS: &[&str] = &["https://mainnet.base.org", "https://base-rpc.publicnode.com"];
/// `getScores(uint256[])`.
const GET_SCORES_SELECTOR: &str = "23d36428";

#[derive(Deserialize)]
struct RpcResponse {
    id: Option<Value>,
    result: Option<Value>,
    error: Option<Value>,
}

/// ABI-encode `getScores(uint256[] fids)`.
pub fn encode_get_scores(fids: &[u64]) -> String {
    let mut data = String::with_capacity(10 + 64 * (fids.len() + 2));
    data.push_str("0x");
    data.push_str(GET_SCORES_SELECTOR);
    data.push_str(&format!("{:064x}", 32));
    data.push_str(&format!("{:064x}", fids.len()));
    for fid in fids {
        data.push_str(&format!("{fid:064x}"));
    }
    data
}

/// Decode a `uint24[]` return value.
pub fn decode_scores(hex: &str, expected: usize) -> Option<Vec<u32>> {
    let hex = hex.strip_prefix("0x")?;
    let word = |i: usize| -> Option<u64> {
        let start = i.checked_mul(64)?;
        let end = start.checked_add(64)?;
        let w = hex.as_bytes().get(start..end)?;
        if !w[..48].iter().all(|b| *b == b'0') {
            return None;
        }
        u64::from_str_radix(std::str::from_utf8(&w[48..]).ok()?, 16).ok()
    };
    // A single dynamic array starts after its one-word return-value head.
    // Check before casting so malformed offsets cannot truncate on wasm32.
    if word(0)? != 32 {
        return None;
    }
    let offset = 1;
    let len = usize::try_from(word(offset)?).ok()?;
    if len != expected {
        return None;
    }
    (0..len).map(|i| word(i.checked_add(offset + 1)?).map(|v| v.min(1_000_000) as u32)).collect()
}

/// fid -> Neynar score in millionths, in batches of up to 150 fids.
pub async fn neynar_scores(inv: &Invocation, fids: &[u64]) -> Result<HashMap<u64, u32>, UpstreamError> {
    let mut out = HashMap::new();
    for batch in fids.chunks(150) {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{ "to": NEYNAR_SCORES_CONTRACT, "data": encode_get_scores(batch) }, "latest"],
        })
        .to_string();
        let mut last_error = UpstreamError::Budget;
        let mut decoded = None;
        for endpoint in BASE_RPCS {
            match rpc::<RpcResponse>(inv, Upstream::BaseRpc, endpoint, &body).await {
                Ok(response) => {
                    decoded = response
                        .result
                        .as_ref()
                        .and_then(Value::as_str)
                        .and_then(|hex| decode_scores(hex, batch.len()));
                    if decoded.is_some() {
                        break;
                    }
                    last_error = UpstreamError::Parse(format!("eth_call failed: {:?}", response.error));
                }
                Err(e @ (UpstreamError::Budget | UpstreamError::Busy(_))) => return Err(e),
                Err(e) => last_error = e,
            }
        }
        let scores = decoded.ok_or(last_error)?;
        out.extend(batch.iter().copied().zip(scores));
    }
    Ok(out)
}

/// The FID's stay vote on Base: `minted(fid)` and its revocation count, in
/// one batched request.
pub async fn vote(inv: &Invocation, fid: u64) -> Result<OnchainVote, UpstreamError> {
    let call = |id: u32, selector: &str| {
        json!({ "jsonrpc": "2.0", "id": id, "method": "eth_call",
                "params": [{ "to": VOTE_CONTRACT, "data": encode_fid_call(selector, fid) }, "latest"] })
    };
    let body = Value::Array(vec![call(0, MINTED_SELECTOR), call(1, REVOKED_SELECTOR)]).to_string();
    let word = |r: &RpcResponse| r.result.as_ref().and_then(Value::as_str).and_then(parse_quantity);
    let mut last_error = UpstreamError::Budget;
    for endpoint in BASE_RPCS {
        match rpc::<Vec<RpcResponse>>(inv, Upstream::BaseRpc, endpoint, &body).await {
            Ok(mut responses) if responses.len() == 2 => {
                responses.sort_by_key(|r| r.id.as_ref().and_then(Value::as_u64).unwrap_or(0));
                if let (Some(minted), Some(revoked)) = (word(&responses[0]), word(&responses[1])) {
                    return Ok(OnchainVote::from_words(minted, revoked));
                }
                last_error = UpstreamError::Parse("vote eth_call failed".into());
            }
            Ok(_) => last_error = UpstreamError::Parse("batch answer size mismatch".into()),
            Err(e @ (UpstreamError::Budget | UpstreamError::Busy(_))) => return Err(e),
            Err(e) => last_error = e,
        }
    }
    Err(last_error)
}

/// Sum of transactions sent (account nonces) by `addresses` on an EVM chain,
/// in one batched JSON-RPC request.
pub async fn evm_sent_count(inv: &Invocation, chain: &ChainInfo, addresses: &[String]) -> Result<u64, UpstreamError> {
    if addresses.is_empty() {
        return Ok(0);
    }
    let batch: Vec<Value> = addresses
        .iter()
        .enumerate()
        .map(|(i, address)| json!({ "jsonrpc": "2.0", "id": i, "method": "eth_getTransactionCount", "params": [address, "latest"] }))
        .collect();
    let body = Value::Array(batch).to_string();
    let mut last_error = UpstreamError::Budget;
    for endpoint in chain.rpcs {
        match rpc::<Vec<RpcResponse>>(inv, Upstream::Chain(chain.slug), endpoint, &body).await {
            // Batch answers may come in any order; only the sum matters.
            Ok(responses) if responses.len() == addresses.len() => {
                let counts: Option<Vec<u64>> = responses
                    .iter()
                    .map(|r| r.result.as_ref().and_then(Value::as_str).and_then(parse_quantity))
                    .collect();
                match counts {
                    Some(counts) => return Ok(counts.iter().sum()),
                    None => last_error = UpstreamError::Parse("eth_getTransactionCount failed".into()),
                }
            }
            Ok(_) => last_error = UpstreamError::Parse("batch answer size mismatch".into()),
            Err(e @ UpstreamError::Budget) => return Err(e),
            Err(e) => last_error = e,
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_decodes_score_calls() {
        let data = encode_get_scores(&[3, 37]);
        assert!(data.starts_with("0x23d36428"));
        assert_eq!(data.len(), 2 + 8 + 64 * 4);
        // Answer captured from mainnet.base.org for fids [3, 37, 3353201, 2, 99].
        let answer = "0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000500000000000000000000000000000000000000000000000000000000000f424000000000000000000000000000000000000000000000000000000000000f4240000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f424000000000000000000000000000000000000000000000000000000000000f4240";
        assert_eq!(decode_scores(answer, 5), Some(vec![1_000_000, 1_000_000, 0, 1_000_000, 1_000_000]));
        assert_eq!(decode_scores(answer, 4), None);
        assert_eq!(decode_scores("0x", 0), None);
    }

    /// Known FIDs, checked against Neynar's score contract on Base on
    /// 2026-10-18 (block 0x324ded7, identical on mainnet.base.org and
    /// publicnode): dwr, v, 0xael.eth, vitalik.eth, rish.
    #[test]
    fn known_fids_score_as_published() {
        let data = encode_get_scores(&[3, 2, 539_854, 5650, 194]);
        assert!(data.ends_with(&format!("{:064x}", 194)));
        let answer = "0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000500000000000000000000000000000000000000000000000000000000000f424000000000000000000000000000000000000000000000000000000000000f424000000000000000000000000000000000000000000000000000000000000b4aa000000000000000000000000000000000000000000000000000000000000ea60000000000000000000000000000000000000000000000000000000000000f4240";
        let scores = decode_scores(answer, 5).unwrap();
        let shown: Vec<String> = scores.iter().map(|m| allthing_core::format::score(*m)).collect();
        assert_eq!(shown, ["1.00", "1.00", "0.74", "0.96", "1.00"]);
    }

    #[test]
    fn malformed_abi_words_return_none_instead_of_panicking() {
        let unicode = format!("0x{}é{}", "0".repeat(47), "0".repeat(15));
        assert_eq!(decode_scores(&unicode, 0), None);
        for offset in [0, 1, 31, 33, u64::MAX] {
            let answer = format!("0x{offset:064x}{:064x}", 0);
            assert_eq!(decode_scores(&answer, 0), None, "accepted invalid offset {offset}");
        }
        assert_eq!(decode_scores(&format!("0x{:064x}{:064x}", 32, 0), 0), Some(vec![]));
    }
}

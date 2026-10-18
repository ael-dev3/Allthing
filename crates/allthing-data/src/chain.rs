// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keyless public JSON-RPC reads: the NS vote (onchain on Base) and
//! EVM and Solana transaction counts.

use allthing_core::vote::{MINT_GENERATION_SELECTOR, MINTED_SELECTOR, OnchainVote, VOTE_CONTRACT, encode_fid_call};
use allthing_core::wallet::{ChainInfo, parse_quantity};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Invocation;
use crate::upstream::{Upstream, UpstreamError, rpc};

const BASE_RPCS: &[&str] = &["https://mainnet.base.org", "https://base-rpc.publicnode.com"];

#[derive(Deserialize)]
struct RpcResponse {
    id: Option<Value>,
    result: Option<Value>,
}

/// The FID's stay vote on Base: `minted(fid)` and `mintGeneration(fid)`
/// (raised by each revoke), in one batched request.
pub async fn vote(inv: &Invocation, fid: u64) -> Result<OnchainVote, UpstreamError> {
    let call = |id: u32, selector: &str| {
        json!({ "jsonrpc": "2.0", "id": id, "method": "eth_call",
                "params": [{ "to": VOTE_CONTRACT, "data": encode_fid_call(selector, fid) }, "latest"] })
    };
    let body = Value::Array(vec![call(0, MINTED_SELECTOR), call(1, MINT_GENERATION_SELECTOR)]).to_string();
    let word = |r: &RpcResponse| r.result.as_ref().and_then(Value::as_str).and_then(parse_quantity);
    let mut last_error = UpstreamError::Budget;
    for endpoint in BASE_RPCS {
        match rpc::<Vec<RpcResponse>>(inv, Upstream::BaseRpc, endpoint, &body).await {
            Ok(mut responses) if responses.len() == 2 => {
                responses.sort_by_key(|r| r.id.as_ref().and_then(Value::as_u64).unwrap_or(0));
                if let (Some(minted), Some(generation)) = (word(&responses[0]), word(&responses[1])) {
                    return Ok(OnchainVote::from_words(minted, generation));
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

/// Signatures one Solana history read returns at most.
pub const SOLANA_PAGE: u64 = 1000;

#[derive(Deserialize)]
struct SignaturesResponse {
    result: Option<Vec<serde::de::IgnoredAny>>,
}

/// Transactions involving `addresses` on Solana: their signatures, up to
/// [`SOLANA_PAGE`] each, one request per address (the public RPC refuses
/// batches). `true` when an address hit the cap, so the real count is higher.
pub async fn solana_tx_count(
    inv: &Invocation,
    chain: &ChainInfo,
    addresses: &[String],
) -> Result<(u64, bool), UpstreamError> {
    let (mut total, mut capped) = (0, false);
    for address in addresses {
        let body = json!({ "jsonrpc": "2.0", "id": 0, "method": "getSignaturesForAddress",
                           "params": [address, { "limit": SOLANA_PAGE }] })
        .to_string();
        let mut answer = Err(UpstreamError::Budget);
        for endpoint in chain.rpcs {
            answer = rpc::<SignaturesResponse>(inv, Upstream::Chain(chain.slug), endpoint, &body).await;
            if answer.is_ok() || matches!(answer, Err(UpstreamError::Budget)) {
                break;
            }
        }
        let count =
            answer?.result.ok_or_else(|| UpstreamError::Parse("getSignaturesForAddress failed".into()))?.len() as u64;
        total += count;
        capped |= count >= SOLANA_PAGE;
    }
    Ok((total, capped))
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use allthing_core::wallet::chain;
    use futures::future::{FutureExt, LocalBoxFuture};

    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    /// Solana's RPC with `per_address` signatures for every address.
    struct Solana {
        per_address: usize,
    }

    impl Host for Solana {
        fn now(&self) -> f64 {
            0.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let call: Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            assert_eq!(call["method"], "getSignaturesForAddress");
            let signatures = vec![json!({ "signature": "s", "slot": 1 }); self.per_address];
            let answer = json!({ "jsonrpc": "2.0", "id": 0, "result": signatures });
            let body = Some(serde_json::to_vec(&answer).unwrap());
            async move { Ok(HttpResponse { status: 200, retry_after: None, body }) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    fn count(per_address: usize, addresses: usize) -> (u64, bool) {
        let inv = Invocation::new(Rc::new(Solana { per_address }), 45);
        let addresses: Vec<String> = (0..addresses).map(|i| format!("addr{i}")).collect();
        solana_tx_count(&inv, chain("solana").unwrap(), &addresses).now_or_never().unwrap().unwrap()
    }

    #[test]
    fn solana_counts_each_addresses_signatures_and_says_when_one_hits_the_cap() {
        assert_eq!(count(7, 2), (14, false));
        assert_eq!(count(SOLANA_PAGE as usize, 1), (SOLANA_PAGE, true));
        assert_eq!(count(3, 0), (0, false));
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! JSON-RPC reads: the NS vote (onchain on Base), whether an address may
//! sign in as a Farcaster account (OP Mainnet), and EVM and Solana
//! transaction counts.

use allthing_core::siwf::{ID_REGISTRY, KEY_REGISTRY, auth_address_call, id_of_answer, id_of_call, is_auth_address};
use allthing_core::vote::{MINT_GENERATION_SELECTOR, MINTED_SELECTOR, OnchainVote, VOTE_CONTRACT, encode_fid_call};
use allthing_core::wallet::{ChainInfo, parse_quantity};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Invocation;
use crate::upstream::{Upstream, UpstreamError, rpc};

const BASE_RPCS: &[&str] = &["https://mainnet.base.org", "https://base-rpc.publicnode.com"];
const OPTIMISM_RPCS: &[&str] = &["https://mainnet.optimism.io", "https://optimism-rpc.publicnode.com"];
const SOLANA_NETWORK_ERROR: &str = "Solana RPC request failed";
const SOLANA_PARSE_ERROR: &str = "Solana RPC returned an invalid response";

/// An older cached wallet error may hold private diagnostics. Only complete
/// known public messages are safe to keep; opaque details become constants.
pub(crate) fn public_solana_error(error: &str) -> std::borrow::Cow<'_, str> {
    let detail = error
        .strip_prefix("upstream unreachable: not measured (")
        .and_then(|detail| detail.strip_suffix(')'))
        .unwrap_or(error);
    let status = detail
        .strip_prefix("upstream returned HTTP ")
        .and_then(|code| code.parse::<u16>().ok())
        .is_some_and(|code| (100..=599).contains(&code));
    let busy = detail
        .strip_prefix("upstream busy, retry in ")
        .and_then(|seconds| seconds.strip_suffix('s'))
        .is_some_and(|seconds| !seconds.is_empty() && seconds.bytes().all(|b| b.is_ascii_digit()));
    if status
        || busy
        || matches!(
            detail,
            "upstream timed out"
                | "request budget exhausted"
                | "upstream unreachable: Solana RPC request failed"
                | "unexpected upstream response: Solana RPC returned an invalid response"
        )
    {
        return std::borrow::Cow::Borrowed(error);
    }
    let public = if detail.starts_with("unexpected upstream response:") {
        UpstreamError::Parse(SOLANA_PARSE_ERROR.into())
    } else {
        UpstreamError::Network(SOLANA_NETWORK_ERROR.into())
    };
    format!("upstream unreachable: not measured ({public})").into()
}

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

/// Whether `address` may sign in as `fid`: it is the account's custody
/// address (`IdRegistry.idOf`) or one of its auth addresses
/// (`KeyRegistry.keyDataOf`), as `@farcaster/auth-client` checks it. Both
/// questions go to OP Mainnet in one batched request.
pub async fn signs_for(inv: &Invocation, fid: u64, address: &str) -> Result<bool, UpstreamError> {
    let call = |id: u32, to: &str, data: String| json!({ "jsonrpc": "2.0", "id": id, "method": "eth_call", "params": [{ "to": to, "data": data }, "latest"] });
    let body = Value::Array(vec![
        call(0, ID_REGISTRY, id_of_call(address)),
        call(1, KEY_REGISTRY, auth_address_call(fid, address)),
    ])
    .to_string();
    let answer = |r: &RpcResponse| r.result.as_ref().and_then(Value::as_str).map(str::to_owned);
    let mut last_error = UpstreamError::Budget;
    for endpoint in OPTIMISM_RPCS {
        match rpc::<Vec<RpcResponse>>(inv, Upstream::OptimismRpc, endpoint, &body).await {
            Ok(mut responses) if responses.len() == 2 => {
                responses.sort_by_key(|r| r.id.as_ref().and_then(Value::as_u64).unwrap_or(0));
                let custody = answer(&responses[0]).and_then(|a| id_of_answer(&a));
                let auth = answer(&responses[1]).and_then(|a| is_auth_address(&a));
                match (custody, auth) {
                    (Some(owner), _) if owner == fid => return Ok(true),
                    (_, Some(true)) => return Ok(true),
                    (Some(_), Some(false)) => return Ok(false),
                    _ => last_error = UpstreamError::Parse("registry eth_call failed".into()),
                }
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
/// Pages read per address on a keyed endpoint: a count up to this many
/// signatures, shown as "5,000+" beyond it.
pub const SOLANA_KEYED_PAGES: usize = 5;

#[derive(Deserialize)]
struct SignaturesResponse {
    result: Option<Vec<Signature>>,
}

#[derive(Deserialize)]
struct Signature {
    signature: String,
}

/// Transactions involving `addresses` on Solana: their signatures, one
/// request per address and page. With a keyed endpoint (see
/// [`Invocation::solana_rpc`]) each address is paged through its history up
/// to [`SOLANA_KEYED_PAGES`] pages; without one, the public endpoint gives
/// [`SOLANA_PAGE`] signatures and refuses batches. `true` when an address hit
/// the cap, so the real count is higher.
pub async fn solana_tx_count(
    inv: &Invocation,
    chain: &ChainInfo,
    addresses: &[String],
) -> Result<(u64, bool), UpstreamError> {
    if let Some(url) = inv.solana_rpc() {
        return solana_keyed_count(inv, &url, addresses).await;
    }
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

/// [`solana_tx_count`] on a keyed endpoint: each address's signatures, page by
/// page, from the newest back. A short page is the end of its history.
async fn solana_keyed_count(inv: &Invocation, url: &str, addresses: &[String]) -> Result<(u64, bool), UpstreamError> {
    let (mut total, mut capped) = (0, false);
    for address in addresses {
        let mut before: Option<String> = None;
        let mut count = 0;
        let mut ended = false;
        for _ in 0..SOLANA_KEYED_PAGES {
            let mut options = json!({ "limit": SOLANA_PAGE });
            if let Some(last) = &before {
                options["before"] = json!(last);
            }
            let body = json!({ "jsonrpc": "2.0", "id": 0, "method": "getSignaturesForAddress",
                               "params": [address, options] })
            .to_string();
            // Transport diagnostics may contain the keyed URL, and parse
            // errors may quote provider content. Wallet errors are public.
            let page: SignaturesResponse =
                rpc(inv, Upstream::Chain("solana-keyed"), url, &body).await.map_err(|error| match error {
                    UpstreamError::Network(_) => UpstreamError::Network(SOLANA_NETWORK_ERROR.into()),
                    UpstreamError::Parse(_) => UpstreamError::Parse(SOLANA_PARSE_ERROR.into()),
                    other => other,
                })?;
            let signatures =
                page.result.ok_or_else(|| UpstreamError::Parse("getSignaturesForAddress failed".into()))?;
            count += signatures.len() as u64;
            if (signatures.len() as u64) < SOLANA_PAGE {
                ended = true;
                break;
            }
            before = signatures.last().map(|s| s.signature.clone());
        }
        total += count;
        capped |= !ended;
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

    /// A keyed Solana endpoint whose every address has `history` signatures,
    /// newest first, named `s0`, `s1`, … and paged by the `before` cursor.
    struct Keyed {
        history: usize,
    }

    impl Host for Keyed {
        fn now(&self) -> f64 {
            1000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let body: Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let options = &body["params"][1];
            let limit = options["limit"].as_u64().unwrap() as usize;
            let start =
                options["before"].as_str().map_or(0, |s| s.trim_start_matches('s').parse::<usize>().unwrap() + 1);
            let page: Vec<Value> =
                (start..self.history.min(start + limit)).map(|i| json!({ "signature": format!("s{i}") })).collect();
            let answer = json!({ "jsonrpc": "2.0", "id": 0, "result": page });
            let body = Some(serde_json::to_vec(&answer).unwrap());
            async move { Ok(HttpResponse { status: 200, retry_after: None, body }) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}

        fn solana_rpc(&self) -> Option<String> {
            Some("https://keyed.example/rpc".into())
        }
    }

    /// OP Mainnet's registries: `idOf` answers `owns` for any address, and
    /// `keyDataOf` answers `(state, key_type)`. With `down`, the first
    /// endpoint fails.
    struct Registries {
        owns: u64,
        key: (u64, u64),
        down: bool,
        calls: std::cell::RefCell<Vec<String>>,
    }

    impl Host for Registries {
        fn now(&self) -> f64 {
            0.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            self.calls.borrow_mut().push(request.url.clone());
            if self.down && request.url == OPTIMISM_RPCS[0] {
                return async { Ok(HttpResponse { status: 502, retry_after: None, body: None }) }.boxed_local();
            }
            let batch: Vec<Value> = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let answers: Vec<Value> = batch
                .iter()
                .rev()
                .map(|call| {
                    let data = call["params"][0]["data"].as_str().unwrap();
                    let result = if data.starts_with("0xd94fe832") {
                        format!("0x{:064x}", self.owns)
                    } else {
                        assert!(data.starts_with("0xac34cc5a"), "{data}");
                        format!("0x{:064x}{:064x}", self.key.0, self.key.1)
                    };
                    json!({ "jsonrpc": "2.0", "id": call["id"], "result": result })
                })
                .collect();
            let body = Some(serde_json::to_vec(&answers).unwrap());
            async move { Ok(HttpResponse { status: 200, retry_after: None, body }) }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    fn signs(owns: u64, key: (u64, u64), down: bool) -> (Result<bool, UpstreamError>, usize) {
        let host = Rc::new(Registries { owns, key, down, calls: Default::default() });
        let inv = Invocation::new(host.clone(), 45);
        let signer = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";
        let answer = signs_for(&inv, 1234, signer).now_or_never().unwrap();
        let calls = host.calls.borrow().len();
        (answer, calls)
    }

    #[test]
    fn the_custody_address_or_an_auth_address_signs_for_an_account() {
        // Custody: idOf names the account. One batched request.
        assert_eq!(signs(1234, (0, 0), false), (Ok(true), 1));
        // An auth address the account added.
        assert_eq!(signs(0, (1, 2), false), (Ok(true), 1));
        // Another account's custody address; a removed auth address; an app key.
        assert_eq!(signs(77, (0, 0), false).0, Ok(false));
        assert_eq!(signs(0, (2, 2), false).0, Ok(false));
        assert_eq!(signs(0, (1, 1), false).0, Ok(false));
        // The first endpoint failing, the second answers.
        assert_eq!(signs(1234, (0, 0), true), (Ok(true), 2));
    }

    fn keyed_count(history: usize, addresses: usize) -> (u64, bool) {
        let inv = Invocation::new(Rc::new(Keyed { history }), 45);
        let addresses: Vec<String> = (0..addresses).map(|i| format!("addr{i}")).collect();
        solana_tx_count(&inv, chain("solana").unwrap(), &addresses).now_or_never().unwrap().unwrap()
    }

    #[test]
    fn a_keyed_endpoint_pages_through_the_whole_history() {
        // 2,500 signatures: three pages, the last one short, so the count is real.
        assert_eq!(keyed_count(2_500, 1), (2_500, false));
        // Two addresses add up.
        assert_eq!(keyed_count(2_500, 2), (5_000, false));
        // No history at all: one empty page.
        assert_eq!(keyed_count(0, 1), (0, false));
    }

    #[test]
    fn a_keyed_history_past_the_page_limit_says_so() {
        // Five full pages without an end: the count stops at the cap, flagged.
        assert_eq!(keyed_count(7_000, 1), (5 * SOLANA_PAGE, true));
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

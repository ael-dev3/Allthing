// SPDX-License-Identifier: AGPL-3.0-or-later
use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use allthing_core::wallet::WalletActivity;
use allthing_data::{FetchError, Host, HttpRequest, HttpResponse, Invocation, pages};
use futures::future::{FutureExt, LocalBoxFuture};
use serde_json::json;

// Synthetic credentials: no real endpoint or secret is used by these tests.
const KEYED: &str = "https://keyed.example.test/rpc?api-key=fixture-private-token";

#[derive(Clone, Copy)]
enum Fault {
    Network,
    Parse,
    Status,
    Timeout,
}

struct Provider {
    fault: Fault,
    reads: Cell<u32>,
}

impl Host for Provider {
    fn now(&self) -> f64 {
        1_800_000_000_000.0
    }

    fn solana_rpc(&self) -> Option<String> {
        Some(KEYED.into())
    }

    fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}

    fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
        self.reads.set(self.reads.get() + 1);
        let result = if request.url.contains("/user/bulk?") {
            Ok(HttpResponse {
                status: 200,
                retry_after: None,
                body: Some(
                    json!({"users": [{"fid": 42, "username": "alice",
                    "verified_addresses": {"sol_addresses": ["fixture-sol-address"]}}]})
                    .to_string()
                    .into_bytes(),
                ),
            })
        } else {
            assert_eq!(request.url, KEYED);
            match self.fault {
                Fault::Network => Err(FetchError::Network(format!("failed fetching {}", request.url))),
                Fault::Parse => Ok(HttpResponse {
                    status: 200,
                    retry_after: None,
                    body: Some(json!({"result": KEYED}).to_string().into_bytes()),
                }),
                Fault::Status => Ok(HttpResponse { status: 503, retry_after: None, body: None }),
                Fault::Timeout => Err(FetchError::Timeout),
            }
        };
        async move { result }.boxed_local()
    }
}

fn wallet_error(fault: Fault, budget: u32) -> String {
    let host = Rc::new(Provider { fault, reads: Cell::new(0) });
    let inv = Rc::new(Invocation::new(host.clone(), budget));
    let built = pages::wallets(inv, 42, None).now_or_never().unwrap().unwrap();
    assert_eq!(built.status, 200);
    let body = String::from_utf8(built.body).unwrap();
    assert!(!body.contains(KEYED), "the public payload contains the private endpoint");
    assert!(!body.contains("fixture-private-token"), "the public payload contains the private key");
    let payload: WalletActivity = serde_json::from_str(&body).unwrap();
    assert_eq!(payload.chains.len(), 1);
    let solana = &payload.chains[0];
    assert_eq!(solana.chain, "solana");
    assert_eq!(solana.count, 0);
    assert_eq!(host.reads.get(), if budget > 1 { 2 } else { 1 });
    solana.error.clone().expect("the unread chain must remain explicit")
}

#[test]
fn keyed_transport_errors_do_not_expose_the_endpoint_in_wallet_payloads() {
    assert!(wallet_error(Fault::Network, 45).contains("Solana RPC request failed"));
}

#[test]
fn keyed_parse_errors_do_not_echo_provider_content_in_wallet_payloads() {
    assert!(wallet_error(Fault::Parse, 45).contains("Solana RPC returned an invalid response"));
}

#[test]
fn keyed_http_status_remains_available_without_the_endpoint() {
    assert!(wallet_error(Fault::Status, 45).contains("HTTP 503"));
}

#[test]
fn keyed_timeout_remains_available_without_the_endpoint() {
    assert!(wallet_error(Fault::Timeout, 45).contains("upstream timed out"));
}

#[test]
fn keyed_budget_failure_remains_available_without_the_endpoint() {
    assert!(wallet_error(Fault::Network, 1).contains("request budget exhausted"));
}

fn cached(error: serde_json::Value) -> serde_json::Value {
    json!({"fid":42,"addresses":[],"chains":[
        {"chain":"base","kind":"evm","name":"Base","count":33000,"percent":100,"carried":true},
        {"chain":"solana","kind":"solana","name":"Solana","count":0,"error":error}
    ],"total":33000,"sources":["fixture source"],"fetched_at":123,
    "extra":{"counter":u64::MAX,"record":"preserve unknown metadata"}})
}

#[test]
fn cached_legacy_diagnostics_are_redacted_without_losing_counts_or_metadata() {
    for detail in [
        format!("upstream unreachable: failed fetching {KEYED}"),
        format!("unexpected upstream response: invalid result {KEYED}"),
        format!("upstream returned HTTP 503; request {KEYED}"),
        format!("upstream busy, retry in 5s; request {KEYED}"),
        "upstream unreachable: rejected fixture-private-token".into(),
    ] {
        let mut original = cached(json!(format!("upstream unreachable: not measured ({detail})")));
        let bytes = serde_json::to_vec(&original).unwrap();
        let public = pages::public_wallet_body(&bytes).unwrap();
        let text = std::str::from_utf8(&public).unwrap();
        assert!(!text.contains(KEYED) && !text.contains("fixture-private-token"));
        let mut after: serde_json::Value = serde_json::from_slice(&public).unwrap();
        assert!(after["chains"][1]["error"].as_str().unwrap().contains("Solana RPC"));
        original["chains"][1].as_object_mut().unwrap().remove("error");
        after["chains"][1].as_object_mut().unwrap().remove("error");
        assert_eq!(after, original, "counts, carried flags and unknown metadata must survive");
    }
}

#[test]
fn cached_safe_diagnostics_remain_useful_and_byte_identical() {
    for detail in [
        "upstream returned HTTP 503",
        "upstream timed out",
        "request budget exhausted",
        "upstream busy, retry in 5s",
        "upstream unreachable: Solana RPC request failed",
        "unexpected upstream response: Solana RPC returned an invalid response",
    ] {
        let body =
            serde_json::to_vec(&cached(json!(format!("upstream unreachable: not measured ({detail})")))).unwrap();
        let public = pages::public_wallet_body(&body).unwrap();
        assert!(matches!(public, Cow::Borrowed(_)));
        assert_eq!(public.as_ref(), body);
    }
}

#[test]
fn cached_successful_counts_keep_the_byte_copy_path() {
    let mut payload = cached(serde_json::Value::Null);
    payload["chains"][1].as_object_mut().unwrap().remove("error");
    let body = serde_json::to_vec(&payload).unwrap();
    assert!(matches!(pages::public_wallet_body(&body).unwrap(), Cow::Borrowed(_)));
}

#[test]
fn cached_unknown_error_shapes_do_not_echo_provider_content() {
    let body = serde_json::to_vec(&cached(json!({"message":KEYED}))).unwrap();
    let public = pages::public_wallet_body(&body).unwrap();
    let text = std::str::from_utf8(&public).unwrap();
    assert!(!text.contains(KEYED) && !text.contains("fixture-private-token"));
}

#[test]
fn cached_malformed_payloads_fail_without_echoing_the_body() {
    for body in [format!("{{\"error\":\"{KEYED}\"}}"), format!("{{\"error\":\"{KEYED}\"")] {
        let failure = pages::public_wallet_body(body.as_bytes()).err().expect("do not serve an unrecognized body");
        assert!(!failure.message.contains(KEYED) && !failure.message.contains("fixture-private-token"));
    }
}

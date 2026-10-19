// SPDX-License-Identifier: AGPL-3.0-or-later
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

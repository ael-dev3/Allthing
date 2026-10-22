// SPDX-License-Identifier: AGPL-3.0-or-later
//! `POST /api/v1/auth/attest`: the Worker's word, to Allthing's SpacetimeDB
//! module, that a browser's SpacetimeDB identity belongs to the Farcaster
//! account signed in here (`allthing_core::stdb`).
//!
//! The Worker signs with its own Ed25519 key, the `ATTEST_KEY` secret (64
//! hex digits), which it uses for nothing else; never the custody key
//! (`ALLTHING_CUSTODY_KEY`). The signing is the runtime's WebCrypto, so no
//! byte of the key is worked in WebAssembly. It holds no reader's key and
//! makes no subrequest.

use allthing_core::stdb::{
    ATTEST_TTL, AttestRequest, Attestation, attestation_message, ed25519_pkcs8, is_identity, parse_attest_key,
};
use worker::js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array, global};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::wasm_bindgen_futures::JsFuture;
use worker::{Env, Request, Response, Result};

use crate::auth::{json, read_json, signed_in_fid};
use crate::error;

/// The `ATTEST_KEY` secret, if it is set and well formed.
fn key(env: &Env) -> Option<[u8; 32]> {
    env.secret("ATTEST_KEY").ok().and_then(|s| parse_attest_key(&s.to_string()))
}

/// Whether this server attests SpacetimeDB identities.
pub fn ready(env: &Env) -> bool {
    key(env).is_some()
}

/// The request's attestation, for a signed-in reader.
pub async fn attest(req: &mut Request, env: &Env, now: i64) -> Result<Response> {
    let Some(fid) = signed_in_fid(req, env, now)? else {
        return error(401, "Sign in to use messages", None);
    };
    let Some(seed) = key(env) else { return error(404, "Messages aren't set up on this server", None) };
    let Some(identity) = read_json::<AttestRequest>(req).await.map(|r| r.identity).filter(|i| is_identity(i)) else {
        return error(400, "Expected {\"identity\": a SpacetimeDB identity, 64 lower-case hex digits}", None);
    };
    let expiry = now.max(0) as u64 + ATTEST_TTL;
    let message = attestation_message(&identity, fid, expiry);
    match sign(&seed, message.as_bytes()).await {
        Some(signature) => json(&Attestation { identity, fid, expiry, signature: hex(&signature) }, &[]),
        None => error(500, "Couldn't sign the attestation", None),
    }
}

/// The public half of `ATTEST_KEY`, as 64 hex digits, for
/// `/api/v1/status`: what the module must pin (spacetime/spacetimedb/src/attest_key.rs).
pub async fn public_key(env: &Env) -> Option<String> {
    let seed = key(env)?;
    let jwk = subtle_call("exportKey", Array::of2(&"jwk".into(), &import(&seed, true).await?)).await?;
    let x = Reflect::get(&jwk, &"x".into()).ok()?.as_string()?;
    Some(hex(&base64url(&x)?))
}

async fn subtle_call(name: &str, args: Array) -> Option<JsValue> {
    let subtle = Reflect::get(&Reflect::get(&global(), &"crypto".into()).ok()?, &"subtle".into()).ok()?;
    let method = Reflect::get(&subtle, &name.into()).ok()?.dyn_into::<Function>().ok()?;
    let promise = Reflect::apply(&method, &subtle, &args).ok()?.dyn_into::<Promise>().ok()?;
    JsFuture::from(promise).await.ok()
}

fn algorithm() -> Option<JsValue> {
    let algorithm = Object::new();
    Reflect::set(&algorithm, &"name".into(), &"Ed25519".into()).ok()?;
    Some(algorithm.into())
}

async fn import(seed: &[u8; 32], extractable: bool) -> Option<JsValue> {
    let der = Uint8Array::from(&ed25519_pkcs8(seed)[..]);
    let usages = Array::of1(&"sign".into());
    let args = Array::of5(&"pkcs8".into(), &der, &algorithm()?, &JsValue::from_bool(extractable), &usages);
    subtle_call("importKey", args).await
}

async fn sign(seed: &[u8; 32], message: &[u8]) -> Option<Vec<u8>> {
    let key = import(seed, false).await?;
    let signature = subtle_call("sign", Array::of3(&algorithm()?, &key, &Uint8Array::from(message))).await?;
    let bytes = Uint8Array::new(&signature).to_vec();
    (bytes.len() == 64).then_some(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpadded base64url, as a JWK's `x` is.
fn base64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_jwk_x() {
        // RFC 8032's first test key's public half, as a JWK spells it.
        let x = base64url("11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo").unwrap();
        assert_eq!(hex(&x), "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        assert_eq!(base64url("a+b"), None);
    }
}

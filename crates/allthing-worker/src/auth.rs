// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sign In With Farcaster, by QR code, through the person's Farcaster app
//! (see `allthing_core::siwf` and docs/ARCHITECTURE.md#sign-in).
//!
//! The browser asks `start` for a nonce, which also goes into a signed,
//! HttpOnly cookie; opens a channel on Farcaster's relay with it; shows the
//! channel's link as a QR code drawn by `qr`; and polls the relay until the
//! person approves in their Farcaster app. It then hands the signed message
//! to `verify`, which checks the message against Farcaster's rules and the
//! nonce in the cookie, recovers its signer from the signature, and asks
//! OP Mainnet whether that signer is the account's custody address or one
//! of its auth addresses. A session cookie, HMAC-signed with the
//! `SESSION_SECRET` Worker secret, then keeps the person signed in;
//! `signout` clears it.
//!
//! Sessions are stateless: changing `SESSION_SECRET` signs everyone out.
//! Nothing here posts anything; signing in only says who is reading.

use std::fmt::Write as _;
use std::rc::Rc;

use allthing_core::model::UserSummary;
use allthing_core::normalize;
use allthing_core::siwf::{
    Expected, PENDING_COOKIE, Proof, QrCode, QrRequest, SESSION_COOKIE, SESSION_TTL, SIGN_IN_TTL, SignInMessage,
    SignedIn, Started, USER_COOKIE, cookie, is_app_link, is_nonce, user_cookie_value,
};
use allthing_data::{Invocation, chain, hypersnap};
use hmac::{Hmac, KeyInit, Mac};
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::Sha256;
use sha3::{Digest, Keccak256};
use worker::{Env, Headers, Method, Request, Response, Result, Url};

use crate::error;

/// What a session cookie's and a pending sign-in's signatures are for, so
/// one can never stand in for the other.
const SESSION: &str = "allthing session";
const PENDING: &str = "allthing sign-in nonce";
/// The largest body a sign-in request may have.
const MAX_BODY: usize = 8 * 1024;
/// A shorter `SESSION_SECRET` is refused as too weak.
const MIN_SECRET: usize = 32;

/// `/api/v1/auth/<name>`. Every call is a same-origin POST, answered
/// uncached.
pub async fn handle(name: &str, mut req: Request, env: &Env, inv: Rc<Invocation>) -> Result<Response> {
    if req.method() != Method::Post {
        return error(405, "Sign-in endpoints take POST", None);
    }
    let url = req.url()?;
    let origin = url.origin().ascii_serialization();
    // Browsers send Origin with every POST: one from another site can't
    // start, finish or end a sign-in here.
    if req.headers().get("origin")?.as_deref() != Some(origin.as_str()) {
        return error(403, "Sign-in requests must come from Allthing itself", None);
    }
    let now = (inv.now() / 1000.0) as i64;
    match name {
        "qr" => match read_json::<QrRequest>(&mut req).await {
            Some(QrRequest { url }) if is_app_link(&url) => match qr_code(&url) {
                Some(code) => json(&code, &[]),
                None => error(400, "That link is too long for a QR code", None),
            },
            _ => error(400, "Expected {\"url\": a Farcaster sign-in link}", None),
        },
        "signout" => json(&serde_json::json!({}), &[&clear(SESSION_COOKIE), &clear(USER_COOKIE)]),
        "start" | "verify" => {
            let Some(secret) = session_secret(env) else {
                return error(503, "Sign-in isn't set up on this server yet", None);
            };
            if name == "start" {
                let Some(nonce) = random_hex(16) else { return error(500, "No randomness for a nonce", None) };
                let (started, pending) = start(&url, secret.as_bytes(), &nonce, now);
                json(&started, &[&pending])
            } else {
                verify(&mut req, &url, secret.as_bytes(), &inv, now).await
            }
        }
        _ => error(404, "Unknown endpoint", None),
    }
}

/// The `SESSION_SECRET` Worker secret, if it is long enough to sign with.
fn session_secret(env: &Env) -> Option<String> {
    env.secret("SESSION_SECRET").ok().map(|s| s.to_string()).filter(|s| s.len() >= MIN_SECRET)
}

/// The site, as the message names it: the request's host, with its port if
/// it has one (`allthing.social`; `wrangler dev` presents requests at the
/// route's host too).
fn domain(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

/// What `start` answers, and the cookie that holds its nonce for `verify`.
fn start(url: &Url, secret: &[u8], nonce: &str, now: i64) -> (Started, String) {
    let pending = seal(secret, PENDING, &format!("{nonce}.{}", now + SIGN_IN_TTL));
    let started = Started { nonce: nonce.to_owned(), domain: domain(url), uri: url.origin().ascii_serialization() };
    (started, set_cookie(PENDING_COOKIE, &pending, SIGN_IN_TTL, true, "Strict"))
}

/// The nonce of the sign-in this browser started, if its cookie is ours
/// and hasn't expired.
fn pending_nonce(secret: &[u8], value: &str, now: i64) -> Option<String> {
    let (nonce, expires) = open(secret, PENDING, value)?.split_once('.')?;
    let fresh = expires.parse::<i64>().ok().is_some_and(|at| now < at);
    (fresh && is_nonce(nonce)).then(|| nonce.to_owned())
}

/// The account a session cookie signs in, if it is ours and current: what
/// endpoints that act for the signed-in account will check (none does yet).
#[cfg(test)]
fn session_fid(secret: &[u8], value: &str, now: i64) -> Option<u64> {
    let (fid, expires) = open(secret, SESSION, value)?.split_once('.')?;
    let current = expires.parse::<i64>().ok().is_some_and(|at| now < at);
    fid.parse().ok().filter(|fid| current && *fid > 0)
}

async fn verify(req: &mut Request, url: &Url, secret: &[u8], inv: &Rc<Invocation>, now: i64) -> Result<Response> {
    let refuse = |message: &str| error(401, message, None);
    let cookies = req.headers().get("cookie")?.unwrap_or_default();
    let Some(nonce) = cookie(&cookies, PENDING_COOKIE).and_then(|value| pending_nonce(secret, value, now)) else {
        return refuse("This sign-in expired; start again");
    };
    let Some(proof) = read_json::<Proof>(req).await else {
        return error(400, "Expected {\"message\", \"signature\"}", None);
    };
    let message = match SignInMessage::parse(&proof.message) {
        Ok(message) => message,
        Err(refused) => return refuse(refused.0),
    };
    let origin = url.origin().ascii_serialization();
    let expected = Expected { domain: &domain(url), origin: &origin, nonce: &nonce };
    let fid = match message.check(&expected, now) {
        Ok(fid) => fid,
        Err(refused) => return refuse(refused.0),
    };
    if recover_signer(&proof.message, &proof.signature).as_deref() != Some(message.address.as_str()) {
        return refuse("The signature doesn't match the signed message");
    }
    match chain::signs_for(inv, fid, &message.address).await {
        Ok(true) => {}
        Ok(false) => return refuse("That address can't sign in as this Farcaster account"),
        Err(_) => return error(503, "Couldn't check the account on Optimism; try again", Some(5)),
    }
    // Who signed in, for the header. The session doesn't depend on it.
    let user = hypersnap::users_bulk(inv, &[fid])
        .await
        .ok()
        .and_then(|users| users.iter().find(|u| u.fid == fid).map(normalize::user_summary))
        .unwrap_or_else(|| UserSummary {
            fid,
            username: format!("fid:{fid}"),
            display_name: String::new(),
            pfp_url: None,
        });
    let session = seal(secret, SESSION, &format!("{fid}.{}", now + SESSION_TTL));
    json(
        &SignedIn { user: user.clone() },
        &[
            &set_cookie(SESSION_COOKIE, &session, SESSION_TTL, true, "Lax"),
            &set_cookie(USER_COOKIE, &user_cookie_value(&user), SESSION_TTL, false, "Lax"),
            &clear(PENDING_COOKIE),
        ],
    )
}

/// The address that signed `message` (EIP-191 `personal_sign`, as Farcaster
/// apps sign), lowercase: what Ethereum's `ecrecover` gives.
fn recover_signer(message: &str, signature: &str) -> Option<String> {
    let bytes = unhex(signature.strip_prefix("0x")?)?;
    // r, s and v; a smart-contract wallet's longer signature isn't an
    // app's.
    let [signature @ .., v] = bytes.as_slice() else { return None };
    if signature.len() != 64 {
        return None;
    }
    let recovery = RecoveryId::from_byte(match v {
        27 | 28 => v - 27,
        0 | 1 => *v,
        _ => return None,
    })?;
    let signature = Signature::from_slice(signature).ok()?;
    let mut prefixed = Keccak256::new();
    prefixed.update(format!("\x19Ethereum Signed Message:\n{}", message.len()).as_bytes());
    prefixed.update(message.as_bytes());
    let key = VerifyingKey::recover_from_prehash(&prefixed.finalize(), &signature, recovery).ok()?;
    let point = key.to_sec1_point(false);
    let hash = Keccak256::digest(&point.as_bytes()[1..]);
    Some(format!("0x{}", hex(&hash[12..])))
}

/// `text` as a QR code: its dark modules as one SVG path, a run of them per
/// rectangle, inside a quiet zone four modules wide.
fn qr_code(text: &str) -> Option<QrCode> {
    const QUIET: usize = 4;
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).ok()?;
    let width = code.width();
    let dark: Vec<bool> = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
    let mut path = String::new();
    for (y, row) in dark.chunks(width).enumerate() {
        let mut x = 0;
        while x < width {
            let run = row[x..].iter().take_while(|d| **d).count();
            if run > 0 {
                let _ = write!(path, "M{} {}h{run}v1h-{run}z", x + QUIET, y + QUIET);
            }
            x += run.max(1);
        }
    }
    Some(QrCode { size: (width + 2 * QUIET) as u32, path })
}

fn mac(secret: &[u8], purpose: &str, value: &str) -> Hmac<Sha256> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret).expect("HMAC takes keys of any length");
    mac.update(purpose.as_bytes());
    mac.update(b"\n");
    mac.update(value.as_bytes());
    mac
}

/// `<value>.<its HMAC-SHA256 for purpose, in hex>`.
fn seal(secret: &[u8], purpose: &str, value: &str) -> String {
    format!("{value}.{}", hex(&mac(secret, purpose, value).finalize().into_bytes()))
}

/// The value `seal` signed, if this secret signed it for this purpose.
fn open<'a>(secret: &[u8], purpose: &str, sealed: &'a str) -> Option<&'a str> {
    let (value, tag) = sealed.rsplit_once('.')?;
    mac(secret, purpose, value).verify_slice(&unhex(tag)?).ok()?;
    Some(value)
}

fn set_cookie(name: &str, value: &str, max_age: i64, http_only: bool, same_site: &str) -> String {
    let http_only = if http_only { "; HttpOnly" } else { "" };
    format!("{name}={value}; Path=/; Max-Age={max_age}; Secure; SameSite={same_site}{http_only}")
}

fn clear(name: &str) -> String {
    set_cookie(name, "", 0, name != USER_COOKIE, "Lax")
}

/// A JSON answer that no cache keeps, setting `cookies`.
fn json<T: Serialize>(value: &T, cookies: &[&str]) -> Result<Response> {
    let headers = Headers::new();
    headers.set("content-type", "application/json; charset=utf-8")?;
    headers.set("cache-control", "no-store")?;
    for set in cookies {
        headers.append("set-cookie", set)?;
    }
    Ok(Response::from_json(value)?.with_headers(headers))
}

/// The request's JSON body, if it is small and well formed.
async fn read_json<T: DeserializeOwned>(req: &mut Request) -> Option<T> {
    let declared = req.headers().get("content-length").ok().flatten()?.parse::<usize>().ok()?;
    if declared > MAX_BODY {
        return None;
    }
    let body = req.text().await.ok()?;
    if body.len() > MAX_BODY {
        return None;
    }
    serde_json::from_str(&body).ok()
}

/// `bytes` random bytes from the runtime's `crypto.getRandomValues`, in hex.
fn random_hex(bytes: u32) -> Option<String> {
    use worker::js_sys::{Function, Reflect, Uint8Array, global};
    use worker::wasm_bindgen::{JsCast, JsValue};
    let crypto = Reflect::get(&global(), &JsValue::from_str("crypto")).ok()?;
    let fill: Function = Reflect::get(&crypto, &JsValue::from_str("getRandomValues")).ok()?.dyn_into().ok()?;
    let array = Uint8Array::new_with_length(bytes);
    fill.call1(&crypto, &array).ok()?;
    Some(hex(&array.to_vec()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn unhex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signed with viem (`account.signMessage`) by Anvil's first development
    /// key, which is public and never funded: a Farcaster app's sign-in to
    /// allthing.social as fid 1234.
    const MESSAGE: &str = "allthing.social wants you to sign in with your Ethereum account:\n\
        0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266\n\n\
        Farcaster Auth\n\n\
        URI: https://allthing.social\n\
        Version: 1\n\
        Chain ID: 10\n\
        Nonce: 8f3c2a1b9d7e4f60a5b3c1d2e0f9a8b7\n\
        Issued At: 2026-10-19T12:00:00.000Z\n\
        Resources:\n\
        - farcaster://fid/1234";
    const SIGNATURE: &str = "0x82f2c228d9aa7bae74b4715b14fb27426e43df1ddb046a4a0a3a96fe4a7fa07243258c559d03e6e68970997a972fd2d5a5cf0e4b0416a8c05f7b06b5b339c8771b";
    const SIGNER: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";
    const SECRET: &[u8] = b"a test secret that is long enough to sign with";

    #[test]
    fn recovers_the_signer_as_ethereum_does() {
        assert_eq!(recover_signer(MESSAGE, SIGNATURE).as_deref(), Some(SIGNER));
        // v as 0/1 instead of 27/28.
        let zero_v = format!("{}00", &SIGNATURE[..SIGNATURE.len() - 2]);
        assert_eq!(recover_signer(MESSAGE, &zero_v).as_deref(), Some(SIGNER));
        // Another message, or a changed signature, has another signer or none.
        let other = MESSAGE.replace("fid/1234", "fid/1235");
        assert_ne!(recover_signer(&other, SIGNATURE).as_deref(), Some(SIGNER));
        let flipped = format!("{}1c", &SIGNATURE[..SIGNATURE.len() - 2]);
        assert_ne!(recover_signer(MESSAGE, &flipped).as_deref(), Some(SIGNER));
        for bad in ["", "0x", "0x1234", &SIGNATURE[2..], &format!("{SIGNATURE}00"), &SIGNATURE.replace("1b", "1d")] {
            assert_eq!(recover_signer(MESSAGE, bad), None, "{bad}");
        }
    }

    #[test]
    fn a_high_s_signature_recovers_the_same_signer() {
        // (r, n - s) with the other recovery id is the same signature to
        // ecrecover.
        const N: [u8; 32] = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0xba, 0xae,
            0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
        ];
        let bytes = unhex(&SIGNATURE[2..]).unwrap();
        let mut s = [0u8; 32];
        let mut borrow = 0i16;
        for i in (0..32).rev() {
            let d = i16::from(N[i]) - i16::from(bytes[32 + i]) - borrow;
            (s[i], borrow) = if d < 0 { ((d + 256) as u8, 1) } else { (d as u8, 0) };
        }
        let high = format!("0x{}{}{:02x}", hex(&bytes[..32]), hex(&s), 27 + (1 - (bytes[64] - 27)));
        assert_eq!(recover_signer(MESSAGE, &high).as_deref(), Some(SIGNER));
    }

    #[test]
    fn sessions_and_nonces_are_sealed() {
        let session = seal(SECRET, SESSION, "1234.2000");
        assert_eq!(session_fid(SECRET, &session, 1999), Some(1234));
        assert_eq!(session_fid(SECRET, &session, 2000), None, "expired");
        assert_eq!(session_fid(b"another secret, also long enough to sign with", &session, 1999), None);
        assert_eq!(session_fid(SECRET, &session.replace("1234.", "1235."), 1999), None, "edited");
        assert_eq!(session_fid(SECRET, "1234.2000", 1999), None, "unsigned");
        // A nonce's seal can't pass for a session's, or the reverse.
        let pending = seal(SECRET, PENDING, "8f3c2a1b9d7e4f60.2000");
        assert_eq!(session_fid(SECRET, &pending, 1999), None);
        assert_eq!(pending_nonce(SECRET, &pending, 1999).as_deref(), Some("8f3c2a1b9d7e4f60"));
        assert_eq!(pending_nonce(SECRET, &pending, 2000), None, "expired");
        assert_eq!(pending_nonce(SECRET, &session, 1999), None);
    }

    #[test]
    fn start_hands_out_the_nonce_and_keeps_it_in_a_cookie() {
        let url = Url::parse("https://allthing.social/api/v1/auth/start").unwrap();
        let (started, cookie_header) = start(&url, SECRET, "8f3c2a1b9d7e4f60a5b3c1d2e0f9a8b7", 1000);
        assert_eq!(started.domain, "allthing.social");
        assert_eq!(started.uri, "https://allthing.social");
        let (pair, attributes) = cookie_header.split_once("; ").unwrap();
        let value = pair.strip_prefix("__Host-allthing-siwf=").unwrap();
        assert_eq!(pending_nonce(SECRET, value, 1000).as_deref(), Some(started.nonce.as_str()));
        assert_eq!(pending_nonce(SECRET, value, 1000 + SIGN_IN_TTL), None);
        assert_eq!(attributes, "Path=/; Max-Age=600; Secure; SameSite=Strict; HttpOnly");
        // In development the port is part of the domain.
        let local = Url::parse("http://localhost:8787/api/v1/auth/start").unwrap();
        assert_eq!(start(&local, SECRET, "8f3c2a1b9d7e4f60", 0).0.domain, "localhost:8787");
    }

    #[test]
    fn cookies_are_secure_and_the_session_is_hidden_from_scripts() {
        assert_eq!(
            set_cookie(SESSION_COOKIE, "v", SESSION_TTL, true, "Lax"),
            "__Host-allthing-session=v; Path=/; Max-Age=2592000; Secure; SameSite=Lax; HttpOnly"
        );
        assert_eq!(clear(USER_COOKIE), "__Host-allthing-user=; Path=/; Max-Age=0; Secure; SameSite=Lax");
        assert!(clear(SESSION_COOKIE).ends_with("; HttpOnly"));
    }

    #[test]
    fn draws_the_qr_code_as_one_path() {
        let link = "https://farcaster.xyz/~/siwf?channelToken=210f1718-427e-46a4-99e3-2207f21f83ec";
        let code = qr_code(link).unwrap();
        // Version 5 at level M holds the link: 37 modules and the quiet zone.
        assert_eq!(code.size, 37 + 8);
        // Each finder pattern's top row is a run of seven dark modules.
        assert!(code.path.starts_with("M4 4h7v1h-7z"), "{}", &code.path[..40]);
        assert!(code.path.contains(&format!("M{} 4h7v1h-7z", 4 + 37 - 7)));
        assert!(code.path.contains(&format!("M4 {}h7v1h-7z", 4 + 37 - 1)));
        assert!(code.path.bytes().all(|b| b.is_ascii_alphanumeric() || b == b' ' || b == b'-'));
        // Drawn back onto a grid, the path is exactly the code's dark
        // modules, inside a light quiet zone, each drawn once.
        let size = code.size as usize;
        let mut drawn = vec![false; size * size];
        for rect in code.path.split_terminator('z') {
            let (x, rest) = rect.strip_prefix('M').unwrap().split_once(' ').unwrap();
            let (y, rest) = rest.split_once('h').unwrap();
            let (run, rest) = rest.split_once("v1h-").unwrap();
            assert_eq!(run, rest);
            let (x, y, run): (usize, usize, usize) = (x.parse().unwrap(), y.parse().unwrap(), run.parse().unwrap());
            for cell in &mut drawn[y * size + x..y * size + x + run] {
                assert!(!*cell);
                *cell = true;
            }
        }
        let modules = qrcode::QrCode::with_error_correction_level(link, qrcode::EcLevel::M).unwrap();
        let dark = modules.to_colors();
        let width = modules.width();
        for y in 0..size {
            for x in 0..size {
                let inside = (4..4 + width).contains(&x) && (4..4 + width).contains(&y);
                let expected = inside && dark[(y - 4) * width + x - 4] == qrcode::Color::Dark;
                assert_eq!(drawn[y * size + x], expected, "module ({x}, {y})");
            }
        }
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(unhex("000fff"), Some(vec![0, 15, 255]));
        assert_eq!(unhex("0g"), None);
        assert_eq!(unhex("abc"), None);
    }
}

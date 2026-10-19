// SPDX-License-Identifier: AGPL-3.0-or-later
//! App keys: how a signed-in reader lets Allthing like, recast and cast for
//! them, as Farcaster's signed key requests work.
//!
//! 1. The browser makes an Ed25519 key pair that can't be exported
//!    (WebCrypto) and keeps it in the reader's browser.
//! 2. The Worker endorses its public key ([`endorse_path`]): a
//!    `SignedKeyRequest`, signed by the custody address of Allthing's own
//!    Farcaster account, which is how the Farcaster app knows which app asks
//!    and how the key registry on OP Mainnet checks it.
//! 3. The browser hands the endorsement to the Farcaster app's API
//!    ([`key_requests_url`]) and shows the request's link as a QR code.
//! 4. The reader approves in their Farcaster app, which adds the key to
//!    their account; the browser polls ([`key_request_url`]) until it has.
//!
//! The key then signs the reader's messages ([`crate::message`]). Allthing's
//! servers never see it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::API_PREFIX;
use crate::params::percent_encode;

/// The Farcaster app's API, which takes signed key requests.
pub const FARCASTER_API: &str = "https://api.farcaster.xyz";
/// How long an endorsement lasts: the reader approves within this, or
/// the key registry refuses the key.
pub const ENDORSEMENT_TTL: i64 = 3600;

/// `POST`: the Worker's endorsement of a key ([`Endorse`] →
/// [`KeyRequest`]), for a signed-in session.
pub fn endorse_path() -> String {
    format!("{API_PREFIX}/auth/signer")
}

/// `GET`: whether this server endorses keys ([`Writes`]); 404 when it
/// doesn't (no `ALLTHING_FID` and `ALLTHING_CUSTODY_KEY` secrets), and then
/// there is nothing to like or recast with.
pub fn writes_path() -> String {
    format!("{API_PREFIX}/signer")
}

/// The answer to [`writes_path`]: the account that asks for readers' keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Writes {
    pub app_fid: u64,
}

/// What the browser asks the Worker to endorse: its public key, `0x` and 64
/// hex digits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endorse {
    pub key: String,
}

/// A signed key request, as the Farcaster app's API takes it: the key, the
/// app that asks, until when, the app's signature and, when the app pays
/// the onchain fee, its sponsorship. The Worker's endorsement is this,
/// passed on as it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyRequest {
    pub key: String,
    pub request_fid: u64,
    pub signature: String,
    pub deadline: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sponsorship: Option<Sponsorship>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sponsorship {
    pub sponsor_fid: u64,
    pub signature: String,
}

/// Where a request to add a key stands, in the Farcaster app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    /// Not approved yet.
    Pending,
    /// Approved by `user_fid`; the key isn't registered yet.
    Approved,
    /// Registered for `user_fid`: the key can sign.
    Completed,
}

/// The Farcaster app's word on a signed key request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requested {
    pub token: String,
    /// The link that opens the request in the Farcaster app.
    pub link: String,
    pub key: String,
    pub state: KeyState,
    /// Who approved it.
    pub user_fid: Option<u64>,
}

/// The Farcaster app's API, where a request is made.
pub fn key_requests_url() -> String {
    format!("{FARCASTER_API}/v2/signed-key-requests")
}

/// The Farcaster app's API, where a request's state is read.
pub fn key_request_url(token: &str) -> String {
    format!("{FARCASTER_API}/v2/signed-key-request?token={}", percent_encode(token))
}

/// The request in the Farcaster app's answer, if it is one. (These answers
/// are read as `serde_json::Value`, as the node's are, so the browser app
/// carries one JSON reader for them all.)
pub fn parse_requested(body: &str) -> Option<Requested> {
    let answer = serde_json::from_str::<Value>(body).ok()?;
    let raw = answer.get("result")?.get("signedKeyRequest")?;
    let text = |name: &str| raw.get(name).and_then(Value::as_str);
    let state = match text("state")? {
        "pending" => KeyState::Pending,
        "approved" => KeyState::Approved,
        "completed" => KeyState::Completed,
        _ => return None,
    };
    let link = text("deeplinkUrl").filter(|link| is_key_request_link(link))?;
    Some(Requested {
        token: text("token")?.to_owned(),
        link: link.to_owned(),
        key: text("key")?.to_ascii_lowercase(),
        state,
        user_fid: raw.get("userFid").and_then(Value::as_u64).filter(|fid| *fid > 0),
    })
}

/// What went wrong, in the Farcaster app's error answer
/// (`{"errors": [{"message": …}]}`).
pub fn api_error(body: &str) -> Option<String> {
    let answer = serde_json::from_str::<Value>(body).ok()?;
    let errors = answer.get("errors")?.as_array()?;
    errors.iter().filter_map(|e| e.get("message")?.as_str()).find(|m| !m.trim().is_empty()).map(str::to_owned)
}

/// The link the Farcaster app opens a key request with:
/// `farcaster://signed-key-request?token=0x…`. The QR endpoint draws these
/// too, and nothing else of the kind.
pub fn is_key_request_link(url: &str) -> bool {
    let Some(token) = url.strip_prefix("farcaster://signed-key-request?token=") else { return false };
    is_token(token)
}

/// A request's token: `0x` and hex digits.
pub fn is_token(token: &str) -> bool {
    token
        .strip_prefix("0x")
        .is_some_and(|hex| (8..=128).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// An Ed25519 public key written as keys are: `0x` and 64 hex digits.
pub fn parse_public_key(key: &str) -> Option<[u8; 32]> {
    let hex = key.strip_prefix("0x")?;
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0xfc0185a4609110983be9ace986f913c28e50092821297f9d";

    #[test]
    fn paths_and_urls() {
        assert_eq!(endorse_path(), "/api/v1/auth/signer");
        assert_eq!(writes_path(), "/api/v1/signer");
        assert_eq!(key_requests_url(), "https://api.farcaster.xyz/v2/signed-key-requests");
        assert_eq!(key_request_url(TOKEN), format!("https://api.farcaster.xyz/v2/signed-key-request?token={TOKEN}"));
        assert_eq!(key_request_url("a&b"), "https://api.farcaster.xyz/v2/signed-key-request?token=a%26b");
    }

    #[test]
    fn a_key_request_goes_out_as_the_farcaster_api_takes_it() {
        let request = KeyRequest {
            key: format!("0x{}", "11".repeat(32)),
            request_fid: 9152,
            signature: "0xabc".into(),
            deadline: 1_760_000_000,
            sponsorship: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "key": request.key, "requestFid": 9152, "signature": "0xabc", "deadline": 1_760_000_000u64 })
        );
        let sponsored =
            KeyRequest { sponsorship: Some(Sponsorship { sponsor_fid: 9152, signature: "0xdef".into() }), ..request };
        assert_eq!(
            serde_json::to_value(&sponsored).unwrap()["sponsorship"],
            serde_json::json!({ "sponsorFid": 9152, "signature": "0xdef" })
        );
    }

    #[test]
    fn reads_the_farcaster_apps_answers() {
        // As api.farcaster.xyz answered on 19 October 2026.
        let pending = format!(
            r#"{{"result":{{"signedKeyRequest":{{"token":"{TOKEN}","deeplinkUrl":"farcaster://signed-key-request?token={TOKEN}","key":"0x1FA7af134a36ad81bc18da12eb1c19ebb2d651f04894239adb96eb1a9d142dd4","requestFid":9152,"state":"pending","isSponsored":false,"keyType":"app-key"}}}}}}"#
        );
        let requested = parse_requested(&pending).unwrap();
        assert_eq!(requested.token, TOKEN);
        assert_eq!(requested.link, format!("farcaster://signed-key-request?token={TOKEN}"));
        assert_eq!(requested.key, "0x1fa7af134a36ad81bc18da12eb1c19ebb2d651f04894239adb96eb1a9d142dd4");
        assert_eq!((requested.state, requested.user_fid), (KeyState::Pending, None));
        let completed = pending.replace(r#""state":"pending""#, r#""state":"completed","userFid":1234"#);
        let requested = parse_requested(&completed).unwrap();
        assert_eq!((requested.state, requested.user_fid), (KeyState::Completed, Some(1234)));
        let approved = pending.replace(r#""state":"pending""#, r#""state":"approved","userFid":1234"#);
        assert_eq!(parse_requested(&approved).unwrap().state, KeyState::Approved);
        assert_eq!(parse_requested(&pending.replace("pending", "revoked")), None);
        assert_eq!(parse_requested(&pending.replace("farcaster://", "https://evil.example/")), None);
        assert_eq!(parse_requested("{}"), None);
        assert_eq!(api_error(r#"{"errors":[{"message":"Invalid signature"}]}"#).as_deref(), Some("Invalid signature"));
        assert_eq!(api_error(&pending), None);
    }

    #[test]
    fn only_key_request_links_and_keys_pass() {
        assert!(is_key_request_link(&format!("farcaster://signed-key-request?token={TOKEN}")));
        for bad in [
            "farcaster://signed-key-request?token=",
            "farcaster://signed-key-request?token=0x12",
            "farcaster://signed-key-request?token=0xzz0185a4609110983be9",
            "farcaster://signed-key-request?token=0xfc0185a46091&next=https://evil.example",
            "https://farcaster.xyz/~/siwf?channelToken=abc",
            "farcaster://other?token=0xfc0185a4609110983be9ace9",
        ] {
            assert!(!is_key_request_link(bad), "{bad}");
        }
        let key = format!("0x{}", "ab".repeat(32));
        assert_eq!(parse_public_key(&key), Some([0xab; 32]));
        assert_eq!(parse_public_key(&key[2..]), None);
        assert_eq!(parse_public_key(&key[..64]), None);
        assert_eq!(parse_public_key(&key.replace("ab", "zz")), None);
    }
}

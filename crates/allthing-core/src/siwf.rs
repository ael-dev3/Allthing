// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sign In With Farcaster, by QR code: what the Worker and the browser app
//! exchange, and the message a person's Farcaster app signs to sign in.
//!
//! That message is a "Sign-In with Ethereum" message (EIP-4361) with
//! Farcaster's rules, checked here as `@farcaster/auth-client` checks it:
//! the Farcaster statement, OP Mainnet's chain id, exactly one
//! `farcaster://fid/<fid>` resource, and the domain and nonce the sign-in
//! was started with. Whether its signer may sign in as that account is for
//! Farcaster's registries on Optimism to say: the calls that ask them are
//! encoded here, and the Worker recovers the signer from the signature
//! (`allthing-worker`'s `auth.rs`, which holds the cryptography, so the
//! browser app never ships it).

use serde::{Deserialize, Serialize};

use crate::api::API_PREFIX;
use crate::model::UserSummary;
use crate::params::{percent_decode, percent_encode};
use crate::time::parse_rfc3339;

/// The statements a Farcaster app signs ("Farcaster Connect" is the older
/// one, which the auth client still accepts).
pub const STATEMENTS: [&str; 2] = ["Farcaster Auth", "Farcaster Connect"];
/// OP Mainnet, where the Farcaster registries live.
pub const CHAIN_ID: u64 = 10;
/// The registry of accounts (fids) and the custody address of each.
pub const ID_REGISTRY: &str = "0x00000000Fc6c5F01Fc30151999387Bb99A9f489b";
/// The registry of the keys accounts add, auth addresses among them.
pub const KEY_REGISTRY: &str = "0x00000000fc1237824fb747abde0ff18990e59b7e";
/// `idOf(address)`: the fid an address holds as its custody address, or 0.
const ID_OF: &str = "d94fe832";
/// `keyDataOf(uint256 fid, bytes key)`: a key's `(state, keyType)`.
const KEY_DATA_OF: &str = "ac34cc5a";
/// `KeyState.ADDED`: a key the account added and hasn't removed.
const KEY_ADDED: u64 = 1;
/// The key type of an auth address, an Ethereum address that may sign in
/// for the account.
const AUTH_ADDRESS: u64 = 2;

/// The relay that carries a sign-in between the browser and the Farcaster
/// app (Farcaster's AuthKit relay).
pub const RELAY: &str = "https://relay.farcaster.xyz";
/// How long a started sign-in waits for the app, in seconds.
pub const SIGN_IN_TTL: i64 = 600;
/// How long a session lasts, in seconds (30 days).
pub const SESSION_TTL: i64 = 30 * 24 * 3600;

/// The session: HttpOnly, so scripts never see it; the Worker signs it.
pub const SESSION_COOKIE: &str = "__Host-allthing-session";
/// The sign-in under way: its nonce, signed, until the app answers.
pub const PENDING_COOKIE: &str = "__Host-allthing-siwf";
/// Who is signed in, for the header to show without asking the Worker.
/// It only labels the page; the session cookie is the sign-in.
pub const USER_COOKIE: &str = "__Host-allthing-user";

pub fn start_path() -> String {
    format!("{API_PREFIX}/auth/start")
}

pub fn qr_path() -> String {
    format!("{API_PREFIX}/auth/qr")
}

pub fn verify_path() -> String {
    format!("{API_PREFIX}/auth/verify")
}

pub fn sign_out_path() -> String {
    format!("{API_PREFIX}/auth/signout")
}

/// `POST /api/v1/auth/start`'s answer: what the browser opens the relay
/// channel with. The nonce also goes into a signed cookie, so only this
/// browser can finish the sign-in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub nonce: String,
    pub domain: String,
    pub uri: String,
}

/// `POST /api/v1/auth/qr`'s body: the relay's link for the Farcaster app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrRequest {
    pub url: String,
}

/// `POST /api/v1/auth/qr`'s answer: the code's dark modules as an SVG path
/// on a `size` × `size` grid, quiet zone included.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrCode {
    pub size: u32,
    pub path: String,
}

/// `POST /api/v1/auth/verify`'s body: what the app signed, as the relay
/// passed it on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    pub message: String,
    pub signature: String,
}

/// `POST /api/v1/auth/verify`'s answer: who signed in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedIn {
    pub user: UserSummary,
}

/// The value of [`USER_COOKIE`].
pub fn user_cookie_value(user: &UserSummary) -> String {
    percent_encode(&serde_json::to_string(user).unwrap_or_default())
}

/// Who [`USER_COOKIE`] says is signed in.
pub fn user_from_cookie(value: &str) -> Option<UserSummary> {
    serde_json::from_str::<UserSummary>(&percent_decode(value)?).ok().filter(|user| user.fid > 0)
}

/// The value of the cookie `name` in a `Cookie` header (or
/// `document.cookie`).
pub fn cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|pair| pair.trim().strip_prefix(name)?.strip_prefix('='))
}

/// A nonce as Sign-In with Ethereum allows it: 8 or more letters and
/// digits (at most 64 here).
pub fn is_nonce(nonce: &str) -> bool {
    (8..=64).contains(&nonce.len()) && nonce.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `0x` and 40 hex digits.
pub fn is_address(address: &str) -> bool {
    address.len() == 42 && address.starts_with("0x") && address[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A link the Farcaster app opens to answer a sign-in: the relay's
/// `https://farcaster.xyz/~/siwf?channelToken=…`, or the older form on
/// warpcast.com. The QR endpoint draws only these, so it can't be used to
/// put any other link in a code shown on Allthing.
pub fn is_app_link(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let Some((host, path)) = rest.split_once('/') else { return false };
    let Some((path, query)) = path.split_once('?') else { return false };
    let token = query.strip_prefix("channelToken=").unwrap_or_default();
    matches!(host, "farcaster.xyz" | "client.farcaster.xyz" | "warpcast.com" | "client.warpcast.com")
        && path.starts_with("~/")
        && path.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'~' | b'/' | b'-'))
        && (8..=128).contains(&token.len())
        && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Why a message can't sign anyone in, in words a person can read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused(pub &'static str);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

const MALFORMED: Refused = Refused("The signed message isn't a sign-in message");
const NOT_FARCASTER: Refused = Refused("The signed message isn't a Farcaster sign-in");
const ELSEWHERE: Refused = Refused("The signed message signs in to another site");
const STALE: Refused = Refused("This sign-in expired or was started in another tab; try again");

/// What a sign-in was started with: the site, and the nonce in this
/// browser's cookie.
pub struct Expected<'a> {
    /// `allthing.social`.
    pub domain: &'a str,
    /// `https://allthing.social`.
    pub origin: &'a str,
    pub nonce: &'a str,
}

/// A parsed sign-in message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignInMessage {
    pub domain: String,
    /// The signer, lowercase.
    pub address: String,
    pub statement: String,
    pub uri: String,
    pub version: String,
    pub chain_id: u64,
    pub nonce: String,
    /// Unix seconds.
    pub issued_at: i64,
    pub expires_at: Option<i64>,
    pub not_before: Option<i64>,
    pub request_id: Option<String>,
    pub resources: Vec<String>,
}

impl SignInMessage {
    /// Read the message's text, line by line as EIP-4361 lays it out:
    ///
    /// ```text
    /// allthing.social wants you to sign in with your Ethereum account:
    /// 0x…
    ///
    /// Farcaster Auth
    ///
    /// URI: https://allthing.social
    /// Version: 1
    /// Chain ID: 10
    /// Nonce: …
    /// Issued At: 2026-10-19T12:00:00.000Z
    /// Resources:
    /// - farcaster://fid/1234
    /// ```
    ///
    /// with optional `Expiration Time`, `Not Before` and `Request ID` lines
    /// before the resources. Farcaster's messages always have a statement.
    pub fn parse(text: &str) -> Result<SignInMessage, Refused> {
        let mut lines = text.split('\n').peekable();
        let first = lines.next().ok_or(MALFORMED)?;
        let site = first.strip_suffix(" wants you to sign in with your Ethereum account:").ok_or(MALFORMED)?;
        // An optional scheme: "https://allthing.social wants you to …".
        let domain = site.split_once("://").map_or(site, |(_, domain)| domain);
        if domain.is_empty() || domain.contains(|c: char| c.is_whitespace() || c == '/') {
            return Err(MALFORMED);
        }
        let address = lines.next().filter(|a| is_address(a)).ok_or(MALFORMED)?.to_ascii_lowercase();
        if lines.next() != Some("") {
            return Err(MALFORMED);
        }
        let statement = lines.next().filter(|s| !s.is_empty() && !s.starts_with("URI: ")).ok_or(MALFORMED)?;
        if lines.next() != Some("") {
            return Err(MALFORMED);
        }
        let mut field = |name: &str| lines.next_if(|line| line.starts_with(name)).map(|line| &line[name.len()..]);
        let time = |value: Option<&str>| value.map(|v| parse_rfc3339(v).ok_or(MALFORMED)).transpose();
        let uri = field("URI: ").ok_or(MALFORMED)?.to_owned();
        let version = field("Version: ").ok_or(MALFORMED)?.to_owned();
        let chain_id = field("Chain ID: ").and_then(|c| c.parse::<u64>().ok()).ok_or(MALFORMED)?;
        let nonce = field("Nonce: ").ok_or(MALFORMED)?.to_owned();
        let issued_at = time(field("Issued At: "))?.ok_or(MALFORMED)?;
        let expires_at = time(field("Expiration Time: "))?;
        let not_before = time(field("Not Before: "))?;
        let request_id = field("Request ID: ").map(str::to_owned);
        let mut resources = Vec::new();
        if field("Resources:") == Some("") {
            while let Some(resource) = field("- ") {
                resources.push(resource.to_owned());
            }
        }
        if lines.next().is_some() {
            return Err(MALFORMED);
        }
        Ok(SignInMessage {
            domain: domain.to_owned(),
            address,
            statement: statement.to_owned(),
            uri,
            version,
            chain_id,
            nonce,
            issued_at,
            expires_at,
            not_before,
            request_id,
            resources,
        })
    }

    /// The fid this message signs in as, if it keeps Farcaster's rules and
    /// answers the sign-in `expected` started. `now` is in Unix seconds.
    /// The nonce, which expires with its cookie, is what keeps an old
    /// message from being used again.
    pub fn check(&self, expected: &Expected, now: i64) -> Result<u64, Refused> {
        if !STATEMENTS.contains(&self.statement.as_str()) || self.version != "1" || self.chain_id != CHAIN_ID {
            return Err(NOT_FARCASTER);
        }
        let same_site = self.uri == expected.origin
            || self.uri.strip_prefix(expected.origin).is_some_and(|rest| rest.starts_with('/'));
        if self.domain != expected.domain || !same_site {
            return Err(ELSEWHERE);
        }
        if self.nonce != expected.nonce {
            return Err(STALE);
        }
        if self.expires_at.is_some_and(|at| now > at) || self.not_before.is_some_and(|at| now < at) {
            return Err(Refused("The signed message isn't valid at this time"));
        }
        let fids: Vec<&str> = self.resources.iter().filter_map(|r| fid_resource(r)).collect();
        match fids.as_slice() {
            [fid] => fid.parse().map_err(|_| NOT_FARCASTER),
            [] => Err(Refused("The signed message names no Farcaster account")),
            _ => Err(Refused("The signed message names more than one Farcaster account")),
        }
    }
}

/// The fid digits of `farcaster://fid/<fid>`, with an optional trailing
/// slash and no leading zero.
fn fid_resource(resource: &str) -> Option<&str> {
    let digits = resource.strip_prefix("farcaster://fid/")?;
    let digits = digits.strip_suffix('/').unwrap_or(digits);
    let shaped = !digits.is_empty() && !digits.starts_with('0') && digits.bytes().all(|b| b.is_ascii_digit());
    shaped.then_some(digits)
}

/// `IdRegistry.idOf(address)`'s call data.
pub fn id_of_call(address: &str) -> String {
    format!("0x{ID_OF}{:0>64}", address.trim_start_matches("0x").to_ascii_lowercase())
}

/// `KeyRegistry.keyDataOf(fid, abi.encode(address))`'s call data: the
/// auth address is looked up as a 32-byte key.
pub fn auth_address_call(fid: u64, address: &str) -> String {
    let key = format!("{:0>64}", address.trim_start_matches("0x").to_ascii_lowercase());
    format!("0x{KEY_DATA_OF}{fid:064x}{:064x}{:064x}{key}", 64, 32)
}

/// The fid in `idOf`'s answer (0 for an address that holds none).
pub fn id_of_answer(answer: &str) -> Option<u64> {
    word(answer, 0).filter(|_| answer.len() == 2 + 64)
}

/// Whether `keyDataOf`'s answer is an auth address the account added and
/// hasn't removed.
pub fn is_auth_address(answer: &str) -> Option<bool> {
    let (state, key_type) = (word(answer, 0)?, word(answer, 1)?);
    (answer.len() == 2 + 128).then_some(state == KEY_ADDED && key_type == AUTH_ADDRESS)
}

/// The `index`th 32-byte word of an ABI-encoded answer, if it fits a u64.
fn word(answer: &str, index: usize) -> Option<u64> {
    let word = answer.strip_prefix("0x")?.get(index * 64..(index + 1) * 64)?;
    let (high, low) = (word.get(..48)?, word.get(48..)?);
    high.bytes().all(|b| b == b'0').then(|| u64::from_str_radix(low, 16).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sign-in for fid 1234 as viem builds it for a Farcaster app (the
    /// Worker's tests check its signature).
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
    const ISSUED: i64 = 1_792_411_200;

    fn expected(nonce: &str) -> Expected<'_> {
        Expected { domain: "allthing.social", origin: "https://allthing.social", nonce }
    }

    #[test]
    fn reads_a_farcaster_sign_in_message() {
        let message = SignInMessage::parse(MESSAGE).unwrap();
        assert_eq!(message.domain, "allthing.social");
        assert_eq!(message.address, "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
        assert_eq!(message.statement, "Farcaster Auth");
        assert_eq!(message.uri, "https://allthing.social");
        assert_eq!((message.version.as_str(), message.chain_id), ("1", 10));
        assert_eq!(message.nonce, "8f3c2a1b9d7e4f60a5b3c1d2e0f9a8b7");
        assert_eq!(message.issued_at, ISSUED);
        assert_eq!(message.resources, ["farcaster://fid/1234"]);
        assert_eq!(message.check(&expected("8f3c2a1b9d7e4f60a5b3c1d2e0f9a8b7"), ISSUED + 30), Ok(1234));
    }

    #[test]
    fn reads_the_optional_lines() {
        // As viem lays them out, with a scheme before the domain.
        let text = "https://allthing.social wants you to sign in with your Ethereum account:\n\
            0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266\n\nFarcaster Auth\n\n\
            URI: https://allthing.social/\nVersion: 1\nChain ID: 10\nNonce: abcdef0123456789\n\
            Issued At: 2026-10-19T12:00:00.000Z\nExpiration Time: 2026-10-19T12:10:00.000Z\n\
            Not Before: 2026-10-19T11:59:00.000Z\nRequest ID: req-1\nResources:\n\
            - farcaster://fid/1234/\n- https://example.com/extra";
        let message = SignInMessage::parse(text).unwrap();
        assert_eq!(message.domain, "allthing.social");
        assert_eq!(message.expires_at, Some(ISSUED + 600));
        assert_eq!(message.not_before, Some(ISSUED - 60));
        assert_eq!(message.request_id.as_deref(), Some("req-1"));
        assert_eq!(message.resources.len(), 2);
        let expected = expected("abcdef0123456789");
        assert_eq!(message.check(&expected, ISSUED), Ok(1234));
        // Outside its window.
        assert!(message.check(&expected, ISSUED + 601).is_err());
        assert!(message.check(&expected, ISSUED - 61).is_err());
    }

    #[test]
    fn refuses_what_isnt_a_sign_in_message() {
        for text in [
            "",
            "hello",
            // No statement.
            &MESSAGE.replace("Farcaster Auth\n\n", "\n"),
            &MESSAGE.replace("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266", "0xf39F"),
            &MESSAGE.replace("Chain ID: 10", "Chain ID: ten"),
            &MESSAGE.replace("Issued At: 2026-10-19T12:00:00.000Z", "Issued At: yesterday"),
            &MESSAGE.replace("\nResources:", "\nSomething: else\nResources:"),
            &format!("{MESSAGE}\n- farcaster://fid/5\nextra"),
            &MESSAGE.replace('\n', "\r\n"),
        ] {
            assert_eq!(SignInMessage::parse(text), Err(MALFORMED), "{text:?}");
        }
    }

    #[test]
    fn keeps_farcasters_rules() {
        let nonce = "8f3c2a1b9d7e4f60a5b3c1d2e0f9a8b7";
        let check = |text: &str| SignInMessage::parse(text).unwrap().check(&expected(nonce), ISSUED);
        assert_eq!(check(&MESSAGE.replace("Farcaster Auth", "Farcaster Connect")), Ok(1234));
        assert_eq!(check(&MESSAGE.replace("Farcaster Auth", "Sign in")), Err(NOT_FARCASTER));
        assert_eq!(check(&MESSAGE.replace("Chain ID: 10", "Chain ID: 1")), Err(NOT_FARCASTER));
        assert_eq!(check(&MESSAGE.replace("Version: 1", "Version: 2")), Err(NOT_FARCASTER));
        // Another site's sign-in, or another of ours.
        assert_eq!(check(&MESSAGE.replace("allthing.social wants", "evil.example wants")), Err(ELSEWHERE));
        assert_eq!(
            check(&MESSAGE.replace("URI: https://allthing.social", "URI: https://evil.example")),
            Err(ELSEWHERE)
        );
        assert_eq!(
            check(&MESSAGE.replace("URI: https://allthing.social", "URI: https://allthing.social.evil.example")),
            Err(ELSEWHERE)
        );
        assert_eq!(check(&MESSAGE.replace(nonce, "0123456789abcdef")), Err(STALE));
        // Exactly one account.
        assert!(check(&MESSAGE.replace("- farcaster://fid/1234", "- https://example.com")).is_err());
        assert!(check(&format!("{MESSAGE}\n- farcaster://fid/5")).is_err());
        for bad in
            ["farcaster://fid/0", "farcaster://fid/01", "farcaster://fid/12a", "farcaster://fid/99999999999999999999"]
        {
            assert!(check(&MESSAGE.replace("farcaster://fid/1234", bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn encodes_the_registry_calls() {
        let address = "0x6B0bDA3F2FfEd5EfC83fa8c024acfF1dD45793f1";
        // Checked live against OP Mainnet: dwr's custody address holds fid 3.
        assert_eq!(id_of_call(address), format!("0xd94fe832{:0>64}", "6b0bda3f2ffed5efc83fa8c024acff1dd45793f1"));
        assert_eq!(
            auth_address_call(3, address),
            format!("0xac34cc5a{:064x}{:064x}{:064x}{:0>64}", 3, 64, 32, "6b0bda3f2ffed5efc83fa8c024acff1dd45793f1")
        );
        assert_eq!(id_of_answer(&format!("0x{:064x}", 3)), Some(3));
        assert_eq!(id_of_answer(&format!("0x{:064x}", 0)), Some(0));
        assert_eq!(id_of_answer("0x"), None);
        assert_eq!(id_of_answer(&format!("0x1{:063x}", 3)), None);
        // (state, keyType): an added auth address; an added app key; nothing.
        assert_eq!(is_auth_address(&format!("0x{:064x}{:064x}", 1, 2)), Some(true));
        assert_eq!(is_auth_address(&format!("0x{:064x}{:064x}", 1, 1)), Some(false));
        assert_eq!(is_auth_address(&format!("0x{:064x}{:064x}", 2, 2)), Some(false));
        assert_eq!(is_auth_address(&format!("0x{:064x}{:064x}", 0, 0)), Some(false));
        assert_eq!(is_auth_address("0x"), None);
    }

    #[test]
    fn app_links_are_the_relays_only() {
        assert!(is_app_link("https://farcaster.xyz/~/siwf?channelToken=210f1718-427e-46a4-99e3-2207f21f83ec"));
        assert!(is_app_link(
            "https://warpcast.com/~/sign-in-with-farcaster?channelToken=210f1718-427e-46a4-99e3-2207f21f83ec"
        ));
        for url in [
            "http://farcaster.xyz/~/siwf?channelToken=210f1718-427e-46a4-99e3-2207f21f83ec",
            "https://evil.example/~/siwf?channelToken=210f1718-427e-46a4-99e3-2207f21f83ec",
            "https://farcaster.xyz.evil.example/~/siwf?channelToken=210f1718-427e-46a4",
            "https://farcaster.xyz/login?channelToken=210f1718-427e-46a4-99e3-2207f21f83ec",
            "https://farcaster.xyz/~/siwf?channelToken=x",
            "https://farcaster.xyz/~/siwf?channelToken=210f1718&next=https://evil.example",
            "https://farcaster.xyz/~/siwf",
        ] {
            assert!(!is_app_link(url), "{url}");
        }
    }

    #[test]
    fn the_user_cookie_round_trips() {
        let user = UserSummary {
            fid: 1234,
            username: "alice.eth".into(),
            display_name: "Alice; \"A\"".into(),
            pfp_url: Some("https://example.com/a b.png".into()),
        };
        let value = user_cookie_value(&user);
        assert!(!value.contains([';', ' ', '"', ',']));
        assert_eq!(user_from_cookie(&value), Some(user));
        let header = format!("theme=dark; {USER_COOKIE}={value}; other=1");
        assert_eq!(cookie(&header, USER_COOKIE), Some(value.as_str()));
        assert_eq!(cookie(&header, "missing"), None);
        assert_eq!(user_from_cookie("garbage"), None);
    }

    #[test]
    fn nonces_and_addresses() {
        assert!(is_nonce("8f3c2a1b9d7e4f60a5b3c1d2e0f9a8b7"));
        assert!(!is_nonce("short"));
        assert!(!is_nonce("has-dash-0123"));
        assert!(is_address("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"));
        assert!(!is_address("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb9226"));
        assert!(!is_address("f39Fd6e51aad88F6F4ce6aB8827279cffFb922661"));
    }
}

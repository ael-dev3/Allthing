// SPDX-License-Identifier: AGPL-3.0-or-later
//! Endorsing a reader's app key (see `allthing_core::app_key`): the
//! `SignedKeyRequest` that the Farcaster app shows as Allthing's request,
//! and that the key registry on OP Mainnet checks before it adds the key.
//!
//! It is an EIP-712 signature by the custody address of the account
//! Allthing asks as over the account's fid, the key and a deadline, in the
//! domain of Farcaster's SignedKeyRequestValidator. The account is
//! `ALLTHING_FID` (`wrangler.toml`: Allthing's own, @allthingapp); its custody
//! key is the `ALLTHING_CUSTODY_KEY` Worker secret, which controls that whole
//! account, and is set in Cloudflare only. With `ALLTHING_SPONSOR` set to
//! `1`, the account also pays the onchain fee, which the Farcaster app takes
//! in warps: a second signature, EIP-191, over the first.
//!
//! The custody key only endorses keys here. Readers' keys are made and kept
//! by their browsers, so the Worker never sees them and can't sign anything
//! as a reader. An endorsement makes no subrequest; it costs one or two
//! secp256k1 signatures.

use allthing_core::app_key::{KeyRequest, Sponsorship};
use allthing_core::provenance::{hex_decode, hex_encode};
use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};
use worker::Env;

/// Farcaster's SignedKeyRequestValidator on OP Mainnet (chain 10).
const VALIDATOR: &str = "0x00000000fc700472606ed4fa22623acf62c60553";
const CHAIN_ID: u64 = 10;

/// The account that asks for readers' keys, and its custody key.
pub struct App {
    pub fid: u64,
    custody: SigningKey,
    sponsor: bool,
}

/// The account, if `ALLTHING_FID` and `ALLTHING_CUSTODY_KEY` are set and
/// sound. Unset, the site has no likes, recasts or casts of its own.
pub fn app(env: &Env) -> Option<App> {
    let read = |value: Option<String>| value.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let fid = read(env.var("ALLTHING_FID").ok().map(|v| v.to_string()))?;
    let key = read(env.secret("ALLTHING_CUSTODY_KEY").ok().map(|s| s.to_string()))?;
    let sponsor = read(env.var("ALLTHING_SPONSOR").ok().map(|v| v.to_string()));
    App::new(&fid, &key, sponsor.as_deref())
}

impl App {
    fn new(fid: &str, custody_key: &str, sponsor: Option<&str>) -> Option<App> {
        let fid = fid.parse::<u64>().ok().filter(|fid| *fid > 0)?;
        let bytes = hex_decode(custody_key).filter(|bytes| bytes.len() == 32)?;
        let custody = SigningKey::from_slice(&bytes).ok()?;
        Some(App { fid, custody, sponsor: matches!(sponsor, Some("1" | "true")) })
    }

    pub fn sponsors(&self) -> bool {
        self.sponsor
    }

    /// The custody key's address, to check that it is the account's (the ID
    /// registry's custody address for `fid`): public, unlike the key.
    pub fn address(&self) -> String {
        let point = self.custody.verifying_key().to_sec1_point(false);
        hex_encode(&Keccak256::digest(&point.as_bytes()[1..])[12..])
    }

    /// The signed key request for `key`, good until `deadline` (Unix
    /// seconds), as the Farcaster app's API takes it.
    pub fn endorse(&self, key: &[u8; 32], deadline: u64) -> KeyRequest {
        let signature = sign(&self.custody, &request_digest(self.fid, key, deadline));
        let sponsorship = self.sponsor.then(|| Sponsorship {
            sponsor_fid: self.fid,
            signature: hex_encode(&sign(&self.custody, &personal_digest(&signature))),
        });
        KeyRequest {
            key: hex_encode(key),
            request_fid: self.fid,
            signature: hex_encode(&signature),
            deadline,
            sponsorship,
        }
    }
}

/// What the custody address signs: EIP-712's
/// `keccak256(0x1901 ‖ domainSeparator ‖ hashStruct(SignedKeyRequest))`.
fn request_digest(request_fid: u64, key: &[u8], deadline: u64) -> [u8; 32] {
    let type_hash = Keccak256::digest(b"SignedKeyRequest(uint256 requestFid,bytes key,uint256 deadline)");
    let mut fields = Keccak256::new();
    fields.update(type_hash);
    fields.update(word(request_fid));
    fields.update(Keccak256::digest(key));
    fields.update(word(deadline));
    let mut digest = Keccak256::new();
    digest.update([0x19, 0x01]);
    digest.update(domain_separator());
    digest.update(fields.finalize());
    digest.finalize().into()
}

/// The validator's EIP-712 domain: its name, version 1, OP Mainnet, and its
/// address.
fn domain_separator() -> [u8; 32] {
    let mut domain = Keccak256::new();
    domain.update(Keccak256::digest(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    ));
    domain.update(Keccak256::digest(b"Farcaster SignedKeyRequestValidator"));
    domain.update(Keccak256::digest(b"1"));
    domain.update(word(CHAIN_ID));
    let mut address = [0u8; 32];
    address[12..].copy_from_slice(&hex_decode(VALIDATOR).unwrap_or_default());
    domain.update(address);
    domain.finalize().into()
}

/// EIP-191's `personal_sign` digest of raw bytes, as a sponsorship signs
/// the request's signature.
fn personal_digest(raw: &[u8]) -> [u8; 32] {
    let mut digest = Keccak256::new();
    digest.update(format!("\x19Ethereum Signed Message:\n{}", raw.len()).as_bytes());
    digest.update(raw);
    digest.finalize().into()
}

/// `value` as an ABI word: 32 bytes, big-endian.
fn word(value: u64) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

/// An Ethereum signature of `digest`: r ‖ s ‖ v, with s in its lower half
/// (which the validator's signature check requires) and v 27 or 28.
fn sign(key: &SigningKey, digest: &[u8; 32]) -> [u8; 65] {
    let (signature, recovery) = key.sign_prehash_recoverable(digest);
    let mut out = [0u8; 65];
    out[..64].copy_from_slice(&signature.to_bytes());
    out[64] = 27 + recovery.to_byte();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hardhat's first test account, which the sign-in tests use too.
    const KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const ADDRESS: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";

    fn address_of(key: &SigningKey) -> String {
        let point = key.verifying_key().to_sec1_point(false);
        hex_encode(&Keccak256::digest(&point.as_bytes()[1..])[12..])
    }

    #[test]
    fn the_status_names_the_custody_address_never_the_key() {
        let app = App::new("3356318", KEY, None).unwrap();
        assert_eq!(app.address(), ADDRESS);
        assert!(!app.address().contains(&KEY[2..]));
    }

    #[test]
    fn the_domain_is_the_validators() {
        // The validator's own `domainSeparatorV4()` on OP Mainnet, read with
        // eth_call on 19 October 2026.
        assert_eq!(
            hex_encode(&domain_separator()),
            "0x5736ad5aa807d8996c35df1ea3226e6929d12494c449524da8f474b972284fd8"
        );
    }

    #[test]
    fn signs_as_viem_signs_the_same_request() {
        // viem 2.21's hashTypedData, signTypedData and signMessage({ raw })
        // for this account, fid 9152, key 0x1111…, deadline 1760000000.
        let app = App::new("9152", KEY, Some("1")).unwrap();
        assert_eq!(address_of(&app.custody), ADDRESS);
        let key = [0x11; 32];
        assert_eq!(
            hex_encode(&request_digest(9152, &key, 1_760_000_000)),
            "0xd8d2dd41a395537e15571c06b7f5e067646766a7de9701272ae2edc4f0869c51"
        );
        let request = app.endorse(&key, 1_760_000_000);
        assert_eq!(request.key, format!("0x{}", "11".repeat(32)));
        assert_eq!((request.request_fid, request.deadline), (9152, 1_760_000_000));
        assert_eq!(
            request.signature,
            "0x9a8a44148ba39cafaaa4fe9fdf1f7544d55f87ffb9762663a67e47e9b1833f7564e350ae6f4bb17c59de61e2833174d90331c8f507d717240bc56339f39afa4d1b"
        );
        let sponsorship = request.sponsorship.unwrap();
        assert_eq!(sponsorship.sponsor_fid, 9152);
        assert_eq!(
            sponsorship.signature,
            "0x656d20621a7546ef58e07b4bac523efa91143513d5f3edd8b5d672bb2db855b50ee1f395f176c73fcb608939c01ba09e859ee2c833c5807676ba1b15a912d9e41b"
        );
    }

    #[test]
    fn signs_what_the_validator_onchain_accepted() {
        // SignedKeyRequestValidator.validate() on OP Mainnet (eth_call, 19
        // October 2026) answered true for this request, and false with its
        // deadline a second later. The test account is the custody address
        // of fid 1607387, which someone registered.
        let app = App::new("1607387", KEY, None).unwrap();
        assert_eq!(
            app.endorse(&[0x22; 32], 1_792_449_898).signature,
            "0x5ca366c8d5a50666dfe817d51a15a7b8c31e67a085e71e5e0387a4f06bd66f2d115f5bada7e1c70f55ee3d98035f2a566d25d4bcefebd2563a1d00fe5e6dd6181b"
        );
    }

    #[test]
    fn the_signature_recovers_the_custody_address() {
        use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
        let app = App::new("1", KEY, None).unwrap();
        for deadline in [1, 1_760_000_000, u64::MAX] {
            let request = app.endorse(&[deadline as u8; 32], deadline);
            assert!(request.sponsorship.is_none());
            let bytes = hex_decode(&request.signature).unwrap();
            let signature = Signature::from_slice(&bytes[..64]).unwrap();
            assert!(signature.normalize_s() == signature, "s is in its lower half");
            let recovery = RecoveryId::from_byte(bytes[64] - 27).unwrap();
            let digest = request_digest(1, &[deadline as u8; 32], deadline);
            let signer = VerifyingKey::recover_from_prehash(&digest, &signature, recovery).unwrap();
            assert_eq!(&signer, app.custody.verifying_key());
        }
    }

    #[test]
    fn needs_a_fid_and_a_custody_key() {
        assert!(App::new("9152", &KEY[2..], None).is_some(), "with or without 0x");
        assert!(App::new("0", KEY, None).is_none());
        assert!(App::new("x", KEY, None).is_none());
        assert!(App::new("9152", &KEY[..64], None).is_none());
        assert!(App::new("9152", &format!("0x{}", "0".repeat(64)), None).is_none(), "zero isn't a key");
        assert!(!App::new("9152", KEY, Some("no")).unwrap().sponsors());
        assert!(App::new("9152", KEY, Some("true")).unwrap().sponsors());
    }
}

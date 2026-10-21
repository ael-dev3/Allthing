// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end encrypted messages between two readers, through Allthing's
//! SpacetimeDB module, which keeps only what this makes: ciphertext, its
//! nonce and the key versions (docs/ARCHITECTURE.md#messages-on-spacetimedb).
//!
//! Each browser has an X25519 key pair that WebCrypto makes unexportable
//! and keeps in IndexedDB; only the public half goes to the module
//! (`set_public_key`, which numbers it: its key version). To write to
//! someone, a browser agrees the X25519 shared secret of its private key
//! and their public key (WebCrypto `deriveBits`), and this module does the
//! rest, in pure Rust:
//!
//! 1. **Key.** HKDF-SHA256 (RFC 5869) of the shared secret, with the salt
//!    `allthing-dm-v1` and, as info, both parties (identity, key version,
//!    public key) in a fixed order: [`key_info`]. Both sides derive the
//!    same 32-byte key, as the secret is the same both ways, so a sender
//!    can read back what they sent. An all-zero secret (a low-order public
//!    key) is refused.
//! 2. **Message.** XChaCha20-Poly1305 with a random 24-byte nonce, over the
//!    text in an envelope padded to a multiple of 32 bytes ([`pad`]), so
//!    the ciphertext's length says little about the text's.
//! 3. **Associated data.** The sender, then the recipient, each with key
//!    version and public key: [`associated_data`]. A message can't be
//!    passed off as going the other way, or as sealed with other keys.
//!
//! The keys are static: anyone who later reads a party's private key from
//! their browser can read all their past messages with it (no forward
//! secrecy), and a new key version starts a new key. The module and the
//! Worker never see a private key or a message's text.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;

/// The scheme's name: HKDF's salt, and the start of its info and of the
/// associated data. A new scheme gets a new name.
pub const SCHEME: &str = "allthing-dm-v1";
/// The longest text a message may have, in bytes.
pub const MAX_TEXT: usize = 4000;
/// XChaCha20's nonce.
pub const NONCE_BYTES: usize = 24;
/// Poly1305's tag, at the end of every ciphertext.
pub const TAG_BYTES: usize = 16;
/// The envelope is padded to a multiple of this many bytes.
pub const PAD_TO: usize = 32;
/// The envelope's version byte.
const ENVELOPE_V1: u8 = 1;

/// One end of a conversation, as the module lists it (`user_key`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Party<'a> {
    /// Its SpacetimeDB identity: 64 lower-case hex digits.
    pub identity: &'a str,
    /// The version of its public key that's used.
    pub key_version: u32,
    /// Its X25519 public key.
    pub public_key: &'a [u8; 32],
}

/// Why a message couldn't be sealed or opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum E2eError {
    /// An identity isn't 64 lower-case hex digits, or both are the same.
    BadParty,
    /// The shared secret is all zeros: the other key is of low order.
    WeakSecret,
    /// The text is empty or longer than [`MAX_TEXT`].
    BadText,
    /// The nonce isn't 24 bytes.
    BadNonce,
    /// The ciphertext doesn't open with this key and associated data: it
    /// was changed, isn't for these parties or keys, or isn't ours.
    Unreadable,
}

impl std::fmt::Display for E2eError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            E2eError::BadParty => "not a conversation between two identities",
            E2eError::WeakSecret => "the other key is unsafe",
            E2eError::BadText => "a message is 1 to 4,000 bytes",
            E2eError::BadNonce => "the nonce isn't 24 bytes",
            E2eError::Unreadable => "this message can't be decrypted",
        })
    }
}

fn is_identity(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `identity:version:public key hex`.
fn describe(party: &Party) -> String {
    format!("{}:{}:{}", party.identity, party.key_version, hex(party.public_key))
}

fn check(a: &Party, b: &Party) -> Result<(), E2eError> {
    if !is_identity(a.identity) || !is_identity(b.identity) || a.identity == b.identity {
        return Err(E2eError::BadParty);
    }
    Ok(())
}

/// HKDF's info: the scheme, then both parties, the one with the lower
/// identity first, so either side derives the same key.
pub fn key_info(a: &Party, b: &Party) -> Vec<u8> {
    let (first, second) = if a.identity <= b.identity { (a, b) } else { (b, a) };
    format!("{SCHEME}|key|{}|{}", describe(first), describe(second)).into_bytes()
}

/// The associated data: the scheme, the sender, then the recipient.
pub fn associated_data(sender: &Party, recipient: &Party) -> Vec<u8> {
    format!("{SCHEME}|from|{}|to|{}", describe(sender), describe(recipient)).into_bytes()
}

/// The 32-byte message key of the X25519 secret `shared` that `a` and `b`
/// agree.
pub fn message_key(shared: &[u8; 32], a: &Party, b: &Party) -> Result<[u8; 32], E2eError> {
    check(a, b)?;
    if shared.iter().all(|b| *b == 0) {
        return Err(E2eError::WeakSecret);
    }
    let mut key = [0; 32];
    Hkdf::<Sha256>::new(Some(SCHEME.as_bytes()), shared)
        .expand(&key_info(a, b), &mut key)
        .map_err(|_| E2eError::WeakSecret)?;
    Ok(key)
}

/// `text` in its envelope: the version byte, its length (2 bytes, big
/// endian), the text, and zeros to a multiple of [`PAD_TO`].
pub fn pad(text: &str) -> Result<Vec<u8>, E2eError> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_TEXT {
        return Err(E2eError::BadText);
    }
    let len = 3 + bytes.len();
    let mut out = Vec::with_capacity(len.div_ceil(PAD_TO) * PAD_TO);
    out.push(ENVELOPE_V1);
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
    out.resize(len.div_ceil(PAD_TO) * PAD_TO, 0);
    Ok(out)
}

/// The text in an envelope, if it is one: a known version, a length that
/// fits, UTF-8, and only zeros after it.
pub fn unpad(envelope: &[u8]) -> Option<String> {
    let (&version, rest) = envelope.split_first()?;
    if version != ENVELOPE_V1 || !envelope.len().is_multiple_of(PAD_TO) || rest.len() < 2 {
        return None;
    }
    let len = u16::from_be_bytes([rest[0], rest[1]]) as usize;
    let (text, padding) = rest[2..].split_at_checked(len)?;
    if len == 0 || len > MAX_TEXT || padding.len() >= PAD_TO || padding.iter().any(|b| *b != 0) {
        return None;
    }
    String::from_utf8(text.to_vec()).ok()
}

/// `text` sealed from `sender` to `recipient`, with the X25519 secret they
/// agree and a fresh random `nonce`: the ciphertext, with its tag.
pub fn seal(
    shared: &[u8; 32],
    sender: &Party,
    recipient: &Party,
    nonce: &[u8; NONCE_BYTES],
    text: &str,
) -> Result<Vec<u8>, E2eError> {
    let key = message_key(shared, sender, recipient)?;
    let envelope = pad(text)?;
    let aad = associated_data(sender, recipient);
    XChaCha20Poly1305::new(&key.into())
        .encrypt(&XNonce::from(*nonce), Payload { msg: &envelope, aad: &aad })
        .map_err(|_| E2eError::BadText)
}

/// The text of a message `sender` sealed to `recipient`, with the X25519
/// secret they agree.
pub fn open(
    shared: &[u8; 32],
    sender: &Party,
    recipient: &Party,
    nonce: &[u8],
    ciphertext: &[u8],
) -> Result<String, E2eError> {
    let nonce: &[u8; NONCE_BYTES] = nonce.try_into().map_err(|_| E2eError::BadNonce)?;
    let key = message_key(shared, sender, recipient)?;
    let aad = associated_data(sender, recipient);
    let envelope = XChaCha20Poly1305::new(&key.into())
        .decrypt(&XNonce::from(*nonce), Payload { msg: ciphertext, aad: &aad })
        .map_err(|_| E2eError::Unreadable)?;
    unpad(&envelope).ok_or(E2eError::Unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use x25519_dalek::{PublicKey, StaticSecret};

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn arr(s: &str) -> [u8; 32] {
        unhex(s).try_into().unwrap()
    }

    // RFC 7748 §6.1: Alice's and Bob's keys and their shared secret.
    const ALICE_PRIVATE: &str = "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a";
    const ALICE_PUBLIC: &str = "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a";
    const BOB_PRIVATE: &str = "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb";
    const BOB_PUBLIC: &str = "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f";
    const SHARED: &str = "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742";
    const ALICE_ID: &str = "c200aaaa00000000000000000000000000000000000000000000000000000001";
    const BOB_ID: &str = "c200bbbb00000000000000000000000000000000000000000000000000000002";

    /// The agreement Allthing leaves to WebCrypto, as RFC 7748 has it.
    #[test]
    fn x25519_agrees_as_rfc_7748_says() {
        let alice = StaticSecret::from(arr(ALICE_PRIVATE));
        let bob = StaticSecret::from(arr(BOB_PRIVATE));
        assert_eq!(PublicKey::from(&alice).to_bytes(), arr(ALICE_PUBLIC));
        assert_eq!(PublicKey::from(&bob).to_bytes(), arr(BOB_PUBLIC));
        assert_eq!(alice.diffie_hellman(&PublicKey::from(arr(BOB_PUBLIC))).to_bytes(), arr(SHARED));
        assert_eq!(bob.diffie_hellman(&PublicKey::from(arr(ALICE_PUBLIC))).to_bytes(), arr(SHARED));
    }

    fn parties<'a>(a: &'a [u8; 32], b: &'a [u8; 32]) -> (Party<'a>, Party<'a>) {
        (
            Party { identity: ALICE_ID, key_version: 1, public_key: a },
            Party { identity: BOB_ID, key_version: 3, public_key: b },
        )
    }

    #[test]
    fn both_sides_derive_one_key_bound_to_both_keys() {
        let (a_pub, b_pub) = (arr(ALICE_PUBLIC), arr(BOB_PUBLIC));
        let (alice, bob) = parties(&a_pub, &b_pub);
        let shared = arr(SHARED);
        let key = message_key(&shared, &alice, &bob).unwrap();
        assert_eq!(key, message_key(&shared, &bob, &alice).unwrap());
        // Pinned, so a change to the scheme can't go unnoticed: the
        // explainer's agents can check it against any HKDF.
        assert_eq!(hex(&key), include_str!("../tests/fixtures/e2e/key.hex").trim());
        let bumped = Party { key_version: 2, ..alice };
        assert_ne!(key, message_key(&shared, &bumped, &bob).unwrap());
        let other = [9; 32];
        assert_ne!(key, message_key(&shared, &Party { public_key: &other, ..alice }, &bob).unwrap());
        assert_eq!(message_key(&[0; 32], &alice, &bob), Err(E2eError::WeakSecret));
        assert_eq!(message_key(&shared, &alice, &alice), Err(E2eError::BadParty));
        let shouting = ALICE_ID.to_uppercase();
        assert_eq!(message_key(&shared, &Party { identity: &shouting, ..alice }, &bob), Err(E2eError::BadParty));
    }

    #[test]
    fn info_and_associated_data_are_as_documented() {
        let (a_pub, b_pub) = (arr(ALICE_PUBLIC), arr(BOB_PUBLIC));
        let (alice, bob) = parties(&a_pub, &b_pub);
        let info = String::from_utf8(key_info(&bob, &alice)).unwrap();
        assert_eq!(info, format!("allthing-dm-v1|key|{ALICE_ID}:1:{ALICE_PUBLIC}|{BOB_ID}:3:{BOB_PUBLIC}"));
        let aad = String::from_utf8(associated_data(&bob, &alice)).unwrap();
        assert_eq!(aad, format!("allthing-dm-v1|from|{BOB_ID}:3:{BOB_PUBLIC}|to|{ALICE_ID}:1:{ALICE_PUBLIC}"));
    }

    #[test]
    fn a_message_round_trips_and_resists_tampering() {
        let (a_pub, b_pub) = (arr(ALICE_PUBLIC), arr(BOB_PUBLIC));
        let (alice, bob) = parties(&a_pub, &b_pub);
        let shared = arr(SHARED);
        let nonce = [7u8; 24];
        let text = "gm 🐸, end to end";
        let sealed = seal(&shared, &alice, &bob, &nonce, text).unwrap();
        assert_eq!(sealed.len(), 32 + TAG_BYTES);
        assert_eq!(hex(&sealed), include_str!("../tests/fixtures/e2e/sealed.hex").trim());
        // Bob reads it, and so does Alice, from her outbox.
        assert_eq!(open(&shared, &alice, &bob, &nonce, &sealed).unwrap(), text);
        // Not the other way round, with other versions, nonce or bytes.
        assert_eq!(open(&shared, &bob, &alice, &nonce, &sealed), Err(E2eError::Unreadable));
        let old = Party { key_version: 2, ..bob };
        assert_eq!(open(&shared, &alice, &old, &nonce, &sealed), Err(E2eError::Unreadable));
        assert_eq!(open(&shared, &alice, &bob, &[8; 24], &sealed), Err(E2eError::Unreadable));
        assert_eq!(open(&shared, &alice, &bob, &nonce[..12], &sealed), Err(E2eError::BadNonce));
        for i in [0, 20, sealed.len() - 1] {
            let mut bad = sealed.clone();
            bad[i] ^= 0x80;
            assert_eq!(open(&shared, &alice, &bob, &nonce, &bad), Err(E2eError::Unreadable));
        }
        assert_eq!(open(&shared, &alice, &bob, &nonce, &sealed[..sealed.len() - 1]), Err(E2eError::Unreadable));
        // Someone else's secret.
        let mut other = shared;
        other[0] ^= 1;
        assert_eq!(open(&other, &alice, &bob, &nonce, &sealed), Err(E2eError::Unreadable));
    }

    #[test]
    fn envelopes_pad_and_check() {
        assert_eq!(pad("a").unwrap().len(), 32);
        assert_eq!(pad(&"a".repeat(29)).unwrap().len(), 32);
        assert_eq!(pad(&"a".repeat(30)).unwrap().len(), 64);
        assert_eq!(pad(&"a".repeat(MAX_TEXT)).unwrap().len(), 4032);
        assert_eq!(pad(""), Err(E2eError::BadText));
        assert_eq!(pad(&"a".repeat(MAX_TEXT + 1)), Err(E2eError::BadText));
        for text in ["a", "hello", &"é".repeat(2000)] {
            assert_eq!(unpad(&pad(text).unwrap()).as_deref(), Some(text));
        }
        let good = pad("hi").unwrap();
        let mut bad = good.clone();
        bad[0] = 2;
        assert_eq!(unpad(&bad), None);
        let mut bad = good.clone();
        bad[31] = 1;
        assert_eq!(unpad(&bad), None);
        let mut bad = good.clone();
        bad[2] = 40;
        assert_eq!(unpad(&bad), None);
        assert_eq!(unpad(&good[..31]), None);
        let mut long = good.clone();
        long.extend([0; 32]);
        assert_eq!(unpad(&long), None);
        assert_eq!(unpad(&[]), None);
        let mut invalid = vec![1, 0, 1, 0xff];
        invalid.resize(32, 0);
        assert_eq!(unpad(&invalid), None);
    }
}

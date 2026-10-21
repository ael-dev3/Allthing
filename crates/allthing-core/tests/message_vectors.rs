// SPDX-License-Identifier: AGPL-3.0-or-later
//! Protocol test vectors for the messages a reader signs through Allthing
//! (`allthing_core::message`): known Farcaster messages, encoded here and
//! checked against their hashes (BLAKE3 of the exact bytes their signer
//! signed, so a hash that matches means the same bytes) and their signers'
//! Ed25519 signatures. Together they cover each message Allthing writes for
//! a like, a recast, a quote and their undoing: `ReactionAdd`,
//! `ReactionRemove`, `CastAdd` with a `CastId` embed, and `CastRemove`.

use allthing_core::message::{
    Body, CastId, EPOCH, Embed, MessageData, NewCast, Parent, Reaction, envelope, message_hash, open_envelope,
    read_data,
};
use allthing_core::provenance::{base64_decode, hex_decode, hex_encode};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::Value;

/// Snapchain's own, from farcasterxyz/snapchain's
/// `src/storage/store/engine_tests.rs` (`entities`, `default_message`): a
/// cast of "msg1", and one of "msg2", by fid 1234 (`FID_FOR_TEST`) at
/// Farcaster time 0 on mainnet (`src/utils/factory.rs`), signed by the test
/// key whose secret is 0x10 and 31 zero bytes (`default_signer`). The test
/// pins the hashes. Ed25519 signs deterministically, so the signatures are
/// the ones Snapchain's factory makes; they were worked out with another
/// implementation (OpenSSL, through Node's crypto).
#[test]
fn snapchains_own_test_vectors() {
    let mut secret = [0u8; 32];
    secret[0] = 0x10;
    let key = SigningKey::from_bytes(&secret);
    assert_eq!(
        hex_encode(key.verifying_key().as_bytes()),
        "0x69104ef20c6676aacc9e8c291800d1b7e911e20f85b4066156ea118ace44b8d7"
    );
    for (text, data, hash, signature) in [
        (
            "msg1",
            "0x080110d20920012a0622046d736731",
            "0xeb1850b43b2dd25935222c9137f5fa71b02b9689",
            "0xaa55132d4e3f0ff24a393e6db6bc8ebf561543d1ba09d9db165ef4465c5a89923aef7007f60d2391cd938615b74c88aa0b5baf83ac63aece8634977cda3b030c",
        ),
        (
            "msg2",
            "0x080110d20920012a0622046d736732",
            "0xee0fcb6344d22ea2af4f97859108eb5a3c6650fd",
            "0xb79fcf0f0adbc726b21dc41596b938f0d40e49e14fed22ee0a3018dd4a793fb4fd3b6a4b0430b05a8bbdd384e5954eacf88200b5eaabcad8428af90f61f1c30e",
        ),
    ] {
        let cast = NewCast { text: text.into(), ..NewCast::default() };
        let bytes = MessageData::new(1234, EPOCH, Body::CastAdd(cast)).encode();
        assert_eq!(hex_encode(&bytes), data);
        let digest = message_hash(&bytes);
        assert_eq!(hex_encode(&digest), hash);
        let signed = key.sign(&digest);
        assert_eq!(hex_encode(&signed.to_bytes()), signature);
        assert!(key.verifying_key().verify(&digest, &signed).is_ok());
    }
}

/// Snapchain's HTTP API reference: `reactionById`'s example response
/// (snapchain.farcaster.xyz/reference/httpapi/reactions, from
/// `site/docs/pages/reference/httpapi/reactions.md`), fid 2's like of fid
/// 1795's cast 0x7363f449…, at Farcaster time 72752656, hashed 0x9fc9c51f….
/// The page shortens the signature and the signer, and the node no longer
/// has the like, so only the hash is checked.
#[test]
fn the_snapchain_docs_example_like() {
    let target = CastId::new(1795, "0x7363f449bfb0e7f01c5a1cc0054768ed5146abc0").unwrap();
    let bytes = MessageData::new(2, EPOCH + 72_752_656, Body::ReactionAdd(Reaction::Like, target)).encode();
    assert_eq!(
        hex_encode(&bytes),
        "0x080310021890bcd82220013a1d0801121908830e12147363f449bfb0e7f01c5a1cc0054768ed5146abc0"
    );
    assert_eq!(hex_encode(&message_hash(&bytes)), "0x9fc9c51f6ea3acb84184efa88ba4f02e7d161766");
}

/// Messages on Snapchain mainnet, verbatim as Hypersnap's node served them
/// on 21 October 2026, each with its URL (`fixtures/snapchain-messages.json`):
/// a like and a recast, an unlike, a quote (a cast embedding another's id)
/// and a deleted cast, as the apps that made them encoded them. Each is built
/// here from what the node says it holds, and must hash as it did and verify
/// against its signer; the envelope the node would be given opens to the
/// same parts.
#[test]
fn messages_on_mainnet() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/snapchain-messages.json")).unwrap();
    let mut types = Vec::new();
    for item in fixture["messages"].as_array().unwrap() {
        let (what, message) = (item["what"].as_str().unwrap(), &item["message"]);
        let data = &message["data"];
        let fid = data["fid"].as_u64().unwrap();
        let read = MessageData::new(fid, EPOCH + data["timestamp"].as_i64().unwrap(), body_of(data));
        let bytes = read.encode();
        types.push(read_data(&bytes).unwrap().message_type);
        let hash = message_hash(&bytes);
        assert_eq!(hex_encode(&hash), message["hash"].as_str().unwrap().to_ascii_lowercase(), "{what}");
        let signer: [u8; 32] = hex_decode(message["signer"].as_str().unwrap()).unwrap().try_into().unwrap();
        let signature: [u8; 64] = base64_decode(message["signature"].as_str().unwrap()).unwrap().try_into().unwrap();
        let verified = VerifyingKey::from_bytes(&signer).unwrap().verify(&hash, &Signature::from_bytes(&signature));
        assert!(verified.is_ok(), "{what}");
        let sealed = envelope(&bytes, &hash, &signature, &signer);
        let opened = open_envelope(&sealed).unwrap();
        assert_eq!(
            (opened.hash, opened.signature, opened.signer, opened.data),
            (&hash[..], &signature[..], &signer[..], &bytes[..]),
            "{what}"
        );
    }
    // A like and a recast, an unlike, a quote, a deleted cast.
    assert_eq!(types, [3, 3, 4, 1, 2]);
}

/// A message's body, from the node's JSON of it.
fn body_of(data: &Value) -> Body {
    let cast_id = |id: &Value| CastId::new(id["fid"].as_u64().unwrap(), id["hash"].as_str().unwrap()).unwrap();
    let reaction = |body: &Value| {
        let kind = match body["type"].as_str().unwrap() {
            "REACTION_TYPE_LIKE" => Reaction::Like,
            "REACTION_TYPE_RECAST" => Reaction::Recast,
            other => panic!("{other}"),
        };
        (kind, cast_id(&body["targetCastId"]))
    };
    match data["type"].as_str().unwrap() {
        "MESSAGE_TYPE_REACTION_ADD" => {
            let (kind, target) = reaction(&data["reactionBody"]);
            Body::ReactionAdd(kind, target)
        }
        "MESSAGE_TYPE_REACTION_REMOVE" => {
            let (kind, target) = reaction(&data["reactionBody"]);
            Body::ReactionRemove(kind, target)
        }
        // The event stream gives the hash in base64.
        "MESSAGE_TYPE_CAST_REMOVE" => Body::CastRemove(
            base64_decode(data["castRemoveBody"]["targetHash"].as_str().unwrap()).unwrap().try_into().unwrap(),
        ),
        "MESSAGE_TYPE_CAST_ADD" => {
            let cast = &data["castAddBody"];
            let numbers = |name: &str| cast[name].as_array().unwrap().iter().map(|n| n.as_u64().unwrap());
            let embeds = cast["embeds"].as_array().unwrap().iter().map(|embed| match embed.get("castId") {
                Some(id) => Embed::Cast(cast_id(id)),
                None => Embed::Url(embed["url"].as_str().unwrap().into()),
            });
            let parent = match (cast["parentCastId"].is_object(), cast["parentUrl"].as_str()) {
                (true, _) => Some(Parent::Cast(cast_id(&cast["parentCastId"]))),
                (false, Some(url)) => Some(Parent::Url(url.into())),
                (false, None) => None,
            };
            Body::CastAdd(NewCast {
                text: cast["text"].as_str().unwrap().into(),
                mentions: numbers("mentions").zip(numbers("mentionsPositions").map(|at| at as u32)).collect(),
                embeds: embeds.collect(),
                parent,
            })
        }
        other => panic!("{other}"),
    }
}

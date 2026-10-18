<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Provenance: which client, which backend

**Status:** design proposal (October 2026). Phase 1 is implemented in Allthing; phases 2 and 3 are proposals for "our chain": a Snapchain-compatible node (Hypersnap or a fork of it) that Allthing's community could run.

Allthing shows two kinds of provenance on every cast:

- **Posted via**: the app that signed the cast.
- **Read from**: the backend Allthing fetched the cast from.

This document explains what the Farcaster protocol records today and what it does not. It also proposes how a node, and then the protocol, could record *client* and *backend (ingress)* provenance in a way that can be verified rather than merely claimed.

Research for this document was done against these revisions:

- farcasterxyz/snapchain `fc87f26` (v0.14.3, 2026-10-15)
- farcasterorg/hypersnap `2e8733d` (v0.13.5, 2026-10-16)
- the farcasterxyz/protocol spec `aa6bdfb`
- farcasterxyz/contracts `main`, as of 2026-10-17

File references below use `S:` for Snapchain and `H:` for Hypersnap.

## 1. What exists today

### 1.1 Messages carry no client or ingress field

A Farcaster message is `MessageData` (type, fid, timestamp, network, body), a 20-byte BLAKE3 `hash` of the data, an Ed25519 `signature`, and the 32-byte public `signer` key. There is no app, client, relay, ingress or receive-time field, either in the signed data or in the envelope (`S:proto/definitions/message.proto:10-53`).

### 1.2 Nodes discard ingress information

A message enters a Snapchain node through one of these paths:

- gRPC `SubmitMessage` or `SubmitBulkMessages`;
- HTTP `POST /v1/submitMessage`;
- the `mempool` gossip topic (relayed from another node);
- the node's own onchain-event and fname connectors.

Each path tags the message with an in-memory `MempoolSource { Gossip, RPC, Local }`. That tag only decides whether to re-gossip the message, and is then dropped (`S:src/mempool/mempool.rs:348-363, 495-512`).

None of the following records which node, peer or client first accepted a message:

- the HTTP accept loop, which discards the remote address (`S:src/network/http_server.rs:5751`);
- the mempool key (kind, timestamp, hash);
- the `Transaction` and `ShardChunk` that commit a message;
- hub events, logs and metrics.

The only place an ingress identity exists is on the wire. Gossipsub runs in `Signed` and `Strict` mode, so each relayed mempool message carries the authenticated PeerId of the node that first published it, which is the node that accepted the submission. Snapchain uses only the immediate forwarder and never persists the publisher (`S:src/network/gossip.rs:351-370, 915-947`).

### 1.3 App attribution exists, per signer key

Apps act for users through signer keys, which reach an account in one of two ways:

- **Onchain.** The KeyRegistry `Add` event's metadata is an ABI-encoded `SignedKeyRequestMetadata { requestFid, requestSigner, signature, deadline }`. The SignedKeyRequestValidator contract checks it: `requestSigner` must be the custody address of `requestFid`, and an EIP-712 signature must cover the key and deadline.
- **Gasless.** Since engine V16 on 2026-05-07, apps can add keys with `KEY_ADD` messages. The node verifies `requestFid` itself and serves it directly (`S:src/network/server.rs:394-461`).

The requesting app's fid is therefore the protocol's answer to "which app posted this?". FIP-7 ("Onchain Signers") and "FIP: Snapchain Signers" (#266) both present it that way, and Snapchain's own metrics tag merged messages with it: fid 9152 is "farcaster" and 309857 is "base" (`S:src/storage/store/engine.rs:2517-2550`).

The attribution has limits:

- It is **per key, not per message**. One key can serve several surfaces or backends of the same app.
- A key that is later removed keeps its old casts (V14, "signer revokes only impact future messages"). Nodes only index *active* keys, so attributing old casts can require the full onchain event history.
- Keys migrated to OP Mainnet in November 2023 were added without validation (`bulkAddKeysForMigration`), so their metadata was never checked by the current registry.

### 1.4 Hypersnap is a Snapchain read node, not a separate chain

Hypersnap forks Snapchain and runs as a **read node on Snapchain mainnet**. It uses:

- the same `FARCASTER_NETWORK_MAINNET`;
- the same validator keys;
- the same bootstrap peers and snapshot bucket;
- the same engine-version schedule through V20 (`H:docker-compose.mainnet.yml`, `H:validators.toml`, `H:docs/hyper.md:10`).

On top of Snapchain it adds:

- a Neynar-shaped `/v2/farcaster` API;
- webhooks;
- a non-pruning "hyper" shadow store;
- a per-block `HyperEnvelope` gossip message, which receivers currently ignore.

Messages submitted to a Hypersnap node are relayed to Snapchain validators over mempool gossip. Every cast Allthing shows therefore lives on the same chain, whichever node received it. Today, "came through Hypersnap or Snapchain" is unknowable after the fact.

## 2. What Allthing does now (phase 1)

Everything here is read-only, needs no protocol change, and uses public endpoints.

1. **Posted via.** For each cast, the Worker reads the cast's `signer` from Hypersnap's `/v1/castById` (one cast) or `/v1/castsByFid` (an author's recent casts). It then maps the key to an app with `/v1/signersByFid`, which lists active onchain and gasless keys:
   - **Gasless keys** carry `requestFid` directly.
   - **Onchain keys** carry KeyRegistry metadata, which `allthing-core::provenance` ABI-decodes.

   Keys that have since been removed fall back to `/v1/onChainEventsByFid?event_type=EVENT_TYPE_SIGNER`. The app fid resolves to its Farcaster account (for example @warpcast). Resolved origins are immutable, so they are cached indefinitely in the Worker, and the unresolved remainder is filled in by later refreshes. [BUDGET.md](BUDGET.md) has the costs.
2. **Read from.** Every payload records the backend that served it. Today that is always Hypersnap (`haatz.quilibrium.com`), a read node mirroring Snapchain mainnet. Each cast carries a badge, and the post page's Provenance box states plainly that the network entry point is *not recorded*.
3. **No guessing.** Allthing never infers an ingress backend from timing, signer or app, because none of those identify the node a message entered through.

## 3. Goals for our chain

| Goal | Meaning |
|---|---|
| **Client attribution** | Each message can name the app that produced it, and nodes can check the claim. |
| **Ingress attribution** | For each message, anyone can learn which node (and so which backend operator) first accepted it, with a signature that relays cannot forge. |
| **Privacy** | Never record or gossip end-user IP addresses or session identifiers; only node identities. |
| **No consensus risk** | Phase 2 must not change what validators agree on; phase 3 changes go through a FIP and an engine-version gate. |
| **Compatibility** | Old nodes keep working: unknown gossip fields are ignored and no message hash changes. |
| **Cheap** | Bounded extra bytes per message; no extra round trips on the submit path. |

## 4. Phase 2: signed ingress receipts (node level, no consensus change)

When a node accepts a message from a client through gRPC, HTTP or a gateway, it issues a receipt signed with its libp2p node key (the same key behind its PeerId):

```proto
// Sketch only. Original to Allthing, not copied from Snapchain's GPL protos.
message IngressReceipt {
  bytes  message_hash  = 1;  // 20-byte BLAKE3 hash of the accepted message
  uint64 fid           = 2;  // the message's fid, for indexing
  bytes  node_peer_id  = 3;  // libp2p PeerId of the accepting node
  string operator      = 4;  // self-declared operator label, e.g. "hypersnap/quilibrium"
  IngressPath path     = 5;  // GRPC, HTTP or GATEWAY
  uint32 received_at   = 6;  // Farcaster time, seconds
  bytes  signature     = 7;  // Ed25519 by the node key over fields 1 to 6, domain-separated
}

enum IngressPath {
  INGRESS_PATH_UNSPECIFIED = 0;
  INGRESS_PATH_GRPC = 1;
  INGRESS_PATH_HTTP = 2;
  INGRESS_PATH_GATEWAY = 3;  // a node-operated submission gateway in front of the node
}
```

How it flows:

1. **Issue on accept.** The submit handlers already know the source is `RPC` before calling the mempool (`S:src/network/server.rs:946-1003`). At that point the node signs a receipt and stores it locally under `(hash) -> IngressReceipt`. Storage is a small column family pruned with hub events (3 days by default) or kept longer by archival nodes. The submit response returns the receipt, so the client app can show "accepted by …".
2. **Gossip alongside the message.** The gossip `MempoolMessage` wrapper is not part of consensus. It can gain an optional `ingress` field next to `user_message` without affecting the message hash, mempool keys or `Transaction` encoding. Receivers verify the signature against the publishing PeerId, which gossipsub already authenticates, and store the *first valid* receipt per hash. Nodes that don't understand the field ignore it, because protobuf skips unknown fields. Hypersnap's existing `HyperEnvelope` and its `ContactInfo.capabilities = ["hyper:v1"]` advertisement are a natural place to negotiate support (`H:src/hyper/mod.rs:61-63`).
3. **Serve it.** A read endpoint, `GET /v1/ingressReceipt?hash=0x…`, returns the stored receipt and whether the publishing PeerId's signature verified. A `/v2` flavor can attach `ingress` to cast objects.
4. **Name the backends.** PeerIds are opaque. Node operators publish `peer_id -> operator` claims; the gossip `ContactInfo` already announces each node's RPC address and version. Allthing ships a small, reviewed registry of known operators (for example Snapchain validators run by Neynar, or Hypersnap at Quilibrium). It labels anything else "unregistered node", and never trusts the self-declared `operator` string on its own.

### What a receipt proves, and what it doesn't

- **It proves** that the node holding that PeerId's key claimed to accept the message at that time, through that path. Relays cannot forge or alter it.
- **It does not prove** that the node was the *first* to see the message. A message submitted to two nodes gets two valid receipts, and receivers keep whichever arrived first. Clients that care should show every receipt they know of.
- **It does not identify the end user.** No IP addresses, cookies or auth usernames are included. The HTTP layer's optional basic-auth username (`S:src/network/rpc_extensions.rs:157-187`) must not be recorded.
- **Sybil nodes** can mint receipts for messages they merely relayed and re-submitted. The operator registry and "first seen" ordering limit how much this matters for display; receipts are provenance hints, not consensus facts.

## 5. Phase 3: protocol changes (via FIP)

### 5.1 Signed client attribution inside `MessageData`

Add an optional field to `MessageData`, numbered past the body `oneof` (field numbers 5 to 24 are taken):

```proto
message ClientTag {
  uint64 app_fid     = 1;  // the app that built this message
  string app_version = 2;  // optional, at most 32 bytes
}
// MessageData { …; ClientTag client = 25; }
```

The tag is covered by the user's signature and committed through the message hash. Nodes enforce one consistency rule: if `client` is present, `client.app_fid` must equal the `requestFid` of the message's `signer` key, whether onchain metadata or gasless record. A malicious app therefore cannot claim another app's fid, and keys without attribution cannot carry a tag.

The rule is enabled by a `ProtocolFeature` and engine version (`S:src/version/version.rs`), so the validation change happens at a coordinated height. The tag also counts toward the 2,048-byte message limit, so it stays small.

**Why bother, when `requestFid` exists?** Because a key's app is a property of the key, and resolving it requires the signer index and, for removed keys, the full event history. A per-message tag makes attribution a property of the message itself, cheap for any indexer and visible in every API. It also makes room for `app_version`.

### 5.2 Optional: commit ingress receipts in shard chunks

Validators could attach the first verified `IngressReceipt` for each user message to the `Transaction`. Two things are needed.

**Validation.** Today `Transaction` contents beyond the replayed state are *not validated*, because replay compares only `shard_root` (`S:src/consensus/proposer.rs:206-259`). The chunk would need a commitment to the receipts, such as a Merkle root in `ShardHeader`, and validators would have to check each receipt's signature.

**Determinism.** Different validators can see different first receipts, so the proposer's choice must simply be accepted as long as it is valid. This choice is non-deterministic, which is why phase 3.2 is optional. Phase 2 already gives readers verifiable receipts without touching consensus.

### 5.3 Per-surface keys (no protocol change)

Apps can already mint a separate signer key per surface or backend and register each with its own `requestFid` or deadline metadata. A "web" key and a "mobile" key, for example, would distinguish clients today with the existing attribution. Allthing would show the distinction automatically.

## 6. How Allthing will show it

| Data | Shown as |
|---|---|
| Signer `requestFid` (today) | "via Farcaster" chip, with the signer key in the tooltip and the Provenance box |
| `ClientTag` (phase 3) | "via Farcaster 4.2", verified against the signer |
| Ingress receipt, operator known (phase 2) | "entered via Hypersnap (Quilibrium), 14:02 UTC", with the signature status |
| Ingress receipt, unknown node | "entered via an unregistered node 12D3Koo…" |
| No receipt | "entry point not recorded" (today's text) |

## 7. Open questions

- Should receipts be kept longer than hub events (3 days) so old casts keep their provenance? Archival nodes could keep them indefinitely at about 120 bytes per message.
- Who curates the operator registry, and how are its entries signed? One option is an onchain mapping from a node key to an operator fid.
- Gateways in front of nodes (Hypersnap's tests mention one, `H:src/core/validations/key_quorum_shared_compat_test.rs:1-19`) could sign their own receipts and include the node receipt. Is a chain of receipts worth the bytes?
- The FIP numbering for gasless signers is unresolved: the Snapchain CLI says "FIP-272", but the discussion is #266.

## 8. Licensing note

Snapchain and Hypersnap are GPL-3.0. GPL-3.0 code may be combined with Allthing's AGPL-3.0-or-later code under GPL-3.0 §13. Even so, the protobuf sketches above are written from scratch rather than copied, and any implementation in our node would follow the upstream projects' licenses.

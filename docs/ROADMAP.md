# Roadmap

This is a sequence of milestones, not a delivery schedule.

## 0 — Runnable seed

- [x] Rust workspace: shared core, Leptos web crate, Axum server.
- [x] Native server rendering and separate WASM hydration.
- [x] Classic light-mode feed with fictional conversations.
- [x] Source/channel filtering, search, new/top sorting, expandable threads, temporary saves.
- [x] Keep per-source counts separate.
- [x] Document the intended product and integration boundaries.

## 1 — Trustworthy live reading

- [ ] Verify current API/RPC capabilities and licensing for each provider.
- [ ] Add one read adapter at a time, with public-data fixtures and bounded requests.
- [ ] Normalize and validate real identities and timestamps.
- [ ] Reconcile overlapping messages while preserving source provenance.
- [ ] Add durable indexing, cursor pagination, freshness, and provider error states.
- [ ] Reconstruct threads, including deleted or unavailable parents.
- [ ] Make channel metadata dependencies explicit.

Done when real feeds and threads remain understandable if either source is slow, absent, or disagrees.

## 2 — Account connection

- [ ] Farcaster QR login with server-side challenge verification.
- [ ] Session lifecycle, nonce replay prevention, secure cookies, logout, and tests.
- [ ] Compatible independent login adapter after its authority model is verified.
- [ ] Account-bound saved posts and user settings.

Done when connecting and disconnecting an account works without requesting write permissions.

## 3 — Deliberate participation

- [ ] Explain and obtain revocable signer authorization separately from login.
- [ ] Support publishing, replying, liking, and recasting where the chosen network accepts them.
- [ ] Handle network-specific message support and failed writes honestly.
- [ ] User-owned mutes and blocks; transparent client moderation controls.

Done when permission, destination, result, and revocation are clear for every write action.

## 4 — Production readiness

- [ ] Measure actual server, payload, hydration, and indexing performance.
- [ ] Test accessibility, mobile behavior, and long/deep/adversarial content.
- [ ] Add deployment hardening, operational visibility, backup/restore, and dependency/license checks.
- [ ] Define public application rules and contribution/release practices.

Hosted CI and deployment can be added when their requirements are defined. The seed currently provides a local check script and does not provision external services.

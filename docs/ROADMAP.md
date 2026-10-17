# Roadmap

This is a sequence of milestones, not a delivery schedule.

The first release is read-only: consume public Snapchain and Hypersnap traffic without authenticating readers or submitting messages.

## 0 — Runnable seed

- [x] Rust workspace: shared core, Leptos web crate, Axum server.
- [x] Native server rendering and separate WASM hydration.
- [x] Classic light-mode feed with fictional conversations.
- [x] Source/channel filtering, search, new/top sorting, expandable threads, temporary saves.
- [x] Keep per-source counts separate.
- [x] Document the intended product and integration boundaries.
- [x] Keep authentication and publishing outside the initial product scope.

## 1 — Trustworthy live reading

- [ ] Verify current API/RPC capabilities and licensing for each provider.
- [ ] Add one read adapter at a time, with public-data fixtures and bounded requests.
- [ ] Normalize and validate real identities and timestamps.
- [ ] Reconcile overlapping messages while preserving source provenance.
- [ ] Add durable indexing, cursor pagination, freshness, and provider error states.
- [ ] Reconstruct threads, including deleted or unavailable parents.
- [ ] Make channel metadata dependencies explicit.
- [ ] Keep the entire reading experience available without authentication.
- [ ] Use read APIs/RPCs only; do not introduce network write capabilities.

Done when real feeds and threads work anonymously and remain understandable if either source is slow, absent, or disagrees.

## 2 — Reading controls

- [ ] Persist reading preferences and saved posts locally in the browser.
- [ ] Add local mutes and content filters with clear controls.
- [ ] Make source freshness and unavailable content easy to understand.
- [ ] Preserve reading position and thread-collapse state where useful.

Done when readers can customize their view without creating accounts or changing protocol state.

## 3 — Read-only production readiness

- [ ] Measure actual server, payload, hydration, and indexing performance.
- [ ] Test accessibility, mobile behavior, and long/deep/adversarial content.
- [ ] Add deployment hardening, operational visibility, backup/restore, and dependency/license checks.
- [ ] Define public application rules and contribution/release practices.

Hosted CI and deployment can be added when their requirements are defined. The seed currently provides a local check script and does not provision external services.

## Deferred — Account features and participation

QR authentication, independent login adapters, app signers, publishing, replies, likes, recasts, and account-bound preferences require a separate scope decision after the read-only client is established. They are not part of the initial implementation plan.

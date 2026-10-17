# Contributing

Allthing is at the seed stage. Small, focused contributions are easiest to review. For large architectural changes, open an issue describing the user problem and proposed approach first.

1. Use the pinned Rust toolchain and cargo-leptos version in the README.
2. Make changes in the relevant crate; keep transport code out of `allthing-core`.
3. Run `./scripts/check.sh` and inspect the interface for changes that affect it.
4. Explain the resulting behavior and validation in your pull request.

Build SSR and hydration separately; do not combine all crate features. Keep sample data explicitly fictional. Do not commit credentials, personal account exports, private keys, local paths, generated build output, or environment files. Preserve third-party license notices and review the license before importing protocol schemas or assets.

Contributions are provided under the repository's MIT license unless explicitly agreed otherwise. Do not include material you do not have permission to redistribute.

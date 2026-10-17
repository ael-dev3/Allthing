# Product brief

## What Allthing should become

A lightweight, independent Farcaster web client for people who prefer reading conversations to navigating a busy dashboard. Bring the clarity of classic Reddit to a shared social protocol: light mode, compact rows, contextual metadata, channels, and nested discussions.

## The reading experience

1. Open a readable front page without signing in.
2. Choose available data sources: Snapchain, Hypersnap, or both.
3. Browse chronologically or use a clearly described ranking option.
4. Filter by channel, find a conversation, and expand its replies.
5. Understand which source supplied a post and which snapshot supplies its counts.
6. Connect an account only when account-specific features are useful.

Channels, membership, moderation, media, and ranking may depend on services beyond the raw message stream. The client must describe these dependencies instead of promising complete parity with another application.

## Authentication and writing

The intended first login is familiar Farcaster QR authentication. A compatible independent flow should be possible through a replaceable authentication adapter when its authority checks and implementation are verified.

Signing in proves account control. Publishing needs a separate authorized app signer. Account connection must explain requested permissions, session lifetime, and revocation. A reader must never have to provide a seed phrase or private custody key to the client.

## Design principles

- Light backgrounds, blue links, restrained accents, and comfortable contrast.
- Dense information with clear hierarchy; fewer decorative containers around posts.
- Replies remain readable as discussions grow, with keyboard-operable collapse controls.
- Source provenance and missing-data states are understandable.
- No imitation network downvotes. Client-owned features are labeled as such.
- User blocks, mutes, filters, and application moderation are distinct from network validity.
- Respect reduced motion and small screens; keep public reading useful before hydration.

## First live milestone

A read-only client with verified adapters, stable cursor pagination, identity reconciliation, source-specific error handling, and tested thread reconstruction. Authentication and writing follow that foundation.

## Outside the seed

Production accounts, private messages, wallet transactions, payments, mini apps, recommendations, and a new consensus protocol are not implemented. A new frontend alone cannot guarantee access to content a provider does not serve or a network does not accept.

# Product brief

## What Allthing should become

A lightweight, independent Farcaster web client for people who prefer reading conversations to navigating a busy dashboard. Bring the clarity of classic Reddit to a shared social protocol: light mode, compact rows, contextual metadata, channels, and nested discussions.

The initial product reads public Snapchain and Hypersnap traffic anonymously. No login or wallet connection is part of this release.

## The reading experience

1. Open a readable front page without signing in.
2. Choose available data sources: Snapchain, Hypersnap, or both.
3. Browse chronologically or use a clearly described ranking option.
4. Filter by channel, find a conversation, and expand its replies.
5. Understand which source supplied a post and which snapshot supplies its counts.
6. Keep reading preferences and filters local to the browser, without an account.

Channels, membership, moderation, media, and ranking may depend on services beyond the raw message stream. The client must describe these dependencies instead of promising complete parity with another application.

## Read-only scope

The client displays public posts, replies, reactions, profiles, and source metadata. Reading a reply tree or showing a like count does not grant permission to reply or react. Source filters, sorting, collapsing threads, and local saves are reading controls.

Do not add authentication flows, account sessions, wallet connections, app-signer registration, or message submission to the initial release. Authentication and participation can be revisited as a separately scoped later phase; they are not dependencies of live reading.

## Design principles

- Light backgrounds, blue links, restrained accents, and comfortable contrast.
- Dense information with clear hierarchy; fewer decorative containers around posts.
- Replies remain readable as discussions grow, with keyboard-operable collapse controls.
- Source provenance and missing-data states are understandable.
- No imitation network downvotes. Client-owned features are labeled as such.
- User blocks, mutes, filters, and application moderation are distinct from network validity.
- Respect reduced motion and small screens; keep public reading useful before hydration.

## First live milestone

A read-only client with verified adapters, stable cursor pagination, identity reconciliation, source-specific error handling, and tested thread reconstruction. Public feeds and threads must work without authenticating a reader.

## Outside the first release

Authentication, publishing, account-bound actions, private messages, wallet transactions, payments, mini apps, recommendations, and a new consensus protocol are outside the initial scope. A new frontend alone cannot guarantee access to content a provider does not serve or a network does not accept.

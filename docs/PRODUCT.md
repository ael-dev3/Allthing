<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Product brief

## What Allthing is

Allthing is a quiet, independent, read-only window onto Farcaster, for people who prefer reading conversations to working through a busy dashboard. It shows what the network is talking about, lets readers follow a thread to its end, and is honest about every number and label it shows.

The name comes from the Allthing in Dan Simmons' *Hyperion Cantos*, the Hegemony's open forum carried over the datasphere. The design borrows that mood: luminous, calm, and a little otherworldly. It keeps a light theme and is not a copy of any other forum's look.

## Principles

1. **Read without an account.** Nobody signs in, connects a wallet or registers a signer. Allthing never writes to the network. Any participation feature would need its own scope decision.
2. **Honest engagement.** Farcaster has likes, recasts and replies, and no downvotes, so Allthing shows those three counts. Ranking is simple and public: likes + 2 × recasts + replies, weighed against age for **hot**. **new** and **top** are also offered.
3. **Never hide anyone.** Labels are shown on the profile they describe, and every cast stays visible. A label from the Farcaster app is that app's opinion, attributed to it and dated; Allthing does not filter on it.
4. **Show where things come from.** Each cast names the app that registered its signing key, and facts like votes and labels name their source. Missing data says so ("not recorded", "not measured", "unavailable") instead of being guessed or hidden.
5. **Media without leaving.** Images open in an accessible viewer. Videos play in place while in view, muted until the reader turns sound on, which then holds for later videos until they mute one. Third-party players load only when asked.
6. **Fast and frugal.** Navigation feels instant ([PERFORMANCE.md](PERFORMANCE.md)). The site is static first, makes one API call per page, and caches at the edge. It runs on a free hosting tier without straining the public data sources it reads.
7. **Accessible by default.** It works with the keyboard throughout, gives images alt text, manages focus in dialogs, respects reduced motion and fits small screens.

## The reading experience

- **Front page**: casts trending across Farcaster, with the author's picture, channel and app, and the three engagement counts. Hovering a count shows who liked, recasted or replied.
- **Search**: one box for casts, people and channels.
- **Channels**: a channel's feed with the same sorting, and a directory with search.
- **Post**: the cast with its media, the parent it replies to, quoted casts, a provenance panel, and threaded replies that collapse and expand.
- **Profile cards**: hovering or tapping a name or profile picture anywhere shows a small card with the start of the bio, followers and following, when the account joined, its Farcaster vote, the Farcaster app's label and wallet activity, and a link to the profile.
- **Profile**: the profile picture, name, bio, followers and following, registration date and Farcaster vote. It also shows the Farcaster app's spam label (or that there is none, or that the data is unavailable), and wallet activity by chain from verified addresses, with a Wallets popover listing the addresses.

## Out of scope for now

Sign-in, posting, reacting, direct messages, wallet transactions, notifications and mini apps are out of scope, as are personal recommendations. Casts that consist of mini apps or frames are left out, since Allthing can't show them, and so is spam by a few fixed content rules: links to `*.vercel.app` apps, anything about gig-bot, DEGEN reward or airdrop bait, and DEGEN staking promotions (yeetr.fun). Allthing cannot show content that its data sources don't serve. When a source is down, Allthing serves the last good copy and says so.

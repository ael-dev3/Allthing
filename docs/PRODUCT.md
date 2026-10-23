<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Product brief

## What Allthing is

Allthing is a quiet, independent window onto Farcaster, made for people who prefer reading conversations to working through a busy dashboard. It shows what the network is talking about, lets readers follow a thread to its end, and is honest about every number and fact it shows.

The name comes from the Allthing in Dan Simmons' *Hyperion Cantos*, the Hegemony's open forum carried over the datasphere. The design borrows that mood: luminous, calm, and a little otherworldly. It keeps a light theme and is not a copy of any other forum's look.

## Principles

1. **Read without an account.** Reading needs no account. Signing in is optional, and nobody connects a wallet. Where Allthing can write for readers, it is one approval in the Farcaster app, by one QR code (or a link on a phone), which signs the reader in and lets them cast, reply, quote, like, recast, follow and delete here: the key stays in their browser, and Allthing writes only when they press a button. A browser without the key, signed in, asks for it with the same dialog on the first write. Only on a server without Allthing's keys does a cast written here open in the Farcaster app instead, which posts it.
2. **Honest engagement.** Farcaster has likes, recasts and replies, and no downvotes, so Allthing shows those three counts. Ranking is simple and public: likes + 2 × recasts + replies, weighed against age for **hot**. **new** and **top** are also offered. A reader's favorites, kept in their browser, weigh 25 times in their own hot and top, and accounts they see less of a 25th (and come later in their new), and nobody else's.
3. **Never hide anyone.** Allthing shows no account labels, and never hides a cast because of who posted it. What it leaves out (mini apps it doesn't run, and spam by a few fixed content rules) is judged by the cast's content. A reader who sees less of an account gets its casts well down their own feeds, never hidden.
4. **Show where things come from.** Each cast names the app that registered its signing key, and facts like votes and wallet activity name their source. Missing data says so ("not recorded", "not measured", "unavailable") instead of being guessed or hidden.
5. **Media without leaving.** Images open in an accessible viewer. Videos play in place while in view, muted until the reader turns sound on, which then holds for later videos until they mute one. Third-party players load only when asked.
6. **Fast and frugal.** Navigation feels instant ([PERFORMANCE.md](PERFORMANCE.md)). The site is static first, makes one API call per page, and caches at the edge. It runs on a free hosting tier without straining the public data sources it reads.
7. **Accessible by default.** It works with the keyboard throughout, gives images alt text, manages focus in dialogs, respects reduced motion and fits small screens.

## The reading experience

- **Front page**: casts trending across Farcaster, with the author's picture, channel and app, and the three engagement counts. Hovering a count, or its icon, shows who liked, recasted or replied. The heart and recast icons like and recast from here for a signed-in reader who approved Allthing once in the Farcaster app; signed out, or until writing from Allthing is switched on, they open the cast in the Farcaster app, to like or recast there (a QR code for the reader's phone on a computer).
- **Allthing's poll**: one question of Allthing's own on the front page, "Would you like an Allthing mobile client?", beside the feed on a wide screen and after its fourth cast on a phone. Anyone can vote, signed in or not, and sees how many voted each way once they have. One vote counts per network, so people who share an address share a vote; Allthing keeps a keyed hash of the network, never the address ([ARCHITECTURE.md](ARCHITECTURE.md#allthings-poll)).
- **Who's reading**: a small "● 12" after Home in the header counts the people reading Allthing now (also shown as a compact dot and count on phones); hovering or clicking it shows the signed-in readers who show themselves, counts everyone else without naming them, and lets a signed-in reader hide ("Show me here").
- **Search**: one box for casts, people and channels.
- **Channels**: a channel's feed with the same sorting, and a directory with search.
- **Post**: the cast with its media, the parent it replies to, quoted casts, a provenance panel, and threaded replies that collapse and expand.
- **Profile cards**: hovering or tapping a name or profile picture anywhere shows a small card with the start of the bio, followers and following, when the account joined, its NS vote and wallet activity, and a link to the profile.
- **Profile**: the profile picture, name, bio, followers and following, registration date and NS vote, Follow for a signed-in reader with a key, and Favorite and Show less for anyone. It also shows wallet activity by chain from verified addresses, with a Wallets popover listing the addresses.

## Out of scope for now

Readers sign in, and like, recast, cast, reply, quote, follow and delete their own casts, with one approval of Allthing's key in the Farcaster app, by one QR code, with a key that stays in their browser. Signed in, they have their notifications: who liked, recasted, replied to, mentioned and quoted their casts, and who followed them. Messages has SpacetimeDB chat and end-to-end encrypted DMs. Wallet transactions and mini apps are out of scope for now, as are personal recommendations. Casts that consist of mini apps or frames are left out, since Allthing can't show them, and so is spam by a few fixed content rules: links to `*.vercel.app` apps, anything about gig-bot, DEGEN reward or airdrop bait, and DEGEN staking promotions (yeetr.fun). Allthing cannot show content that its data sources don't serve. When a source is down, Allthing serves the last good copy and says so.

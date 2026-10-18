<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# Changelog

All notable changes to Allthing are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (0.x while it is a prototype; see [docs/RELEASING.md](docs/RELEASING.md)).

## [Unreleased]

- Changing the front-page time range starts that range's pagination immediately. Requests and retry timers from the previous range cannot block it, hide its loading state, or append an obsolete page.

### Added

- Video sound sticks: once the reader turns a video's sound on (its badge, its controls, or a click to play it), later videos autoplay with sound until they mute one. The choice is kept in the browser's local storage. The "Sound off · tap for sound" badge on an autoplaying video is now a button that turns its sound on or off in place. Where the browser won't play sound before the reader has interacted with the page, the video plays muted and its sound comes on at their first tap or key press.

### Changed

- Profiles say when the account joined. Accounts registered before the ID registry moved to OP Mainnet in November 2023 showed "before Nov 2023", the earliest date onchain data has for them; they now show when they got their first username (dwr: 17 Jun 2021), read from the fname server's public transfer log, which the old sign-up registered along with the account. If that read fails, the profile keeps the date its previous copy had and is rebuilt within a minute. Newer accounts show their onchain registration, as before.
- The front page has a Reddit-style time filter next to hot and top: All time, Past year, Past month, Past week, Today (the default) and Past hour, kept in the URL as `t`. It picks the window Hypersnap ranks trending casts over: 1 hour, 24 hours or 7 days. Hypersnap goes back 7 days at most, so Past month, Past year and All time show the past week's casts and say so. A link with a time range still costs one API request, started by the inline script.
- Pull requests run deterministic browser regressions against the local build, alongside the Rust checks. Browser failure reports are retained for 7 days; live production performance checks keep their separate job.
- Feeds show each cast's whole text, with its paragraphs, line breaks, links and mentions, instead of its first 120 characters as a one-line title. A cast with only media shows just its media, without an "[image]" or "[video]" placeholder.
- The front page's data request starts from a small inline script in `index.html` as soon as the page arrives, alongside the WebAssembly download, instead of after the app starts. It is still one API call.
- Media is bigger. A single image or video takes its own shape, as wide as the card and at most 70% of the window tall (80% when portrait), instead of at most 580 px wide and 460 px tall. Its frame fits it, so a narrow image has no bars beside it, and a portrait video is no longer letterboxed in a 16:10 box. Back to a feed still lays images out at their size at once, and now videos too.
- Pages no longer say "Updated N min ago". The notice that a cached copy is shown because the data source isn't answering stays.
- Search results stay compact: the first 6 people and 8 channels, laid out in columns with a count on each group, and "Show all" to see the rest. Casts start above the fold, and their text no longer takes the group headings' display font.
- The header's Channels button and the front page's Channels sidebar are gone: the feed is one centered column, and the Worker no longer looks up trending channels for it. Channels are still found through search and channel links, and `/channels` still lists them.
- The README is rewritten, with a wordmark for light and dark themes, CI, license and release badges, the features, the architecture and a quick start. A code of conduct, issue forms for bugs and feature requests and a pull request template join refreshed contributing and security guides, stale screenshots are gone, and `.gitignore` covers what local wrangler and Playwright runs leave behind.

### Removed

- Neynar scores are gone: no score badges beside names in feeds, posts, replies, popovers and search, and no Neynar score on profiles. The Worker no longer reads scores from Neynar's contract on Base, which saves up to 2 subrequests per page build.

### Fixed

- DEGEN staking promotions are left out as spam, like DEGEN reward bait: casts linking to or naming yeetr.fun, and "stake your DEGEN" or "yield on your DEGEN" pitches. Casts that just discuss staking stay.
- "new" on the front page lists the newest casts on Farcaster, seconds to a minute old. It used to re-sort the trending pool, whose newest cast was hours old: Hypersnap ranks trending casts with a lag by design (even its 1-hour window's casts are 1–2 hours old), and Hypersnap's search index was about 8 hours behind. "new" now comes from Snapchain's event stream (`/api/v1/latest`): each build reads only the events since the previous one, adds the new top-level casts and drops removed ones, and the list is rebuilt at the edge every 30 seconds. `/status` reports how old the newest cast in the new and trending lists is, and how old each list is.
- Muted text (times, handles, counts, the "read-only" pill) is easier to read: its color now has at least 4.5:1 contrast on cards and on the page background (it was 4.0:1 on cards and 3.7:1 on the background). Post pages have a level-one heading for screen readers ("Cast by …"), and the posting app's link in "Posted via" is underlined instead of told apart by color alone.
- The static test server answers malformed paths with 400 instead of crashing, and rejects paths into sibling directories whose names share its root prefix.
- Copying a verified wallet waits for the clipboard write before showing success, and reports failures with a retry instead of falsely saying “Copied”.
- Profiles preserve unknown vote states when the leave-vote source is unavailable or records disagree, and identify app-reported revocations without claiming they were confirmed onchain.
- Reaction dialogs keep keyboard focus inside, restore it when dismissed, and lock background scrolling. Their profile links use the app router, and navigating or going Back closes the dialog.
- Profiles say how an account voted in only three ways: "Voted yes" (stay), "Voted no" (leave) or "Didn't vote". A stay vote that Network School's moderation revoked counts as not voting, since revoking cancels it on Base. When the sources can't settle it, the vote is left out instead of saying "Unknown", and while it loads an empty pill replaces "checking…".
- Card text can be selected: dragging across it selects it instead of dragging a link, and doesn't open the post, while a plain click still does. The post's link for keyboards and screen readers is the timestamp.
- The YouTube and Vimeo click-to-load thumbnail no longer enlarges or brightens on hover. Media isn't highlighted or enlarged on hover anywhere.
- Clicking a card outside its links opened the post only after about 4 seconds. It used a view transition whose promise waited for an animation frame, which never comes during a transition, so Chrome's timeout ended it. Cards now open posts directly, in under 100 ms, with a short CSS fade-in.
- The docs and the stylesheet no longer mention or use view transitions, which cards stopped using in 0.3.x; the leftover cross-document `@view-transition` rule is gone too.
- The changelog's 0.1.0 and 0.2.0 entries link to their release tags (`v0.1.0`, `v0.2.0`) and compare views instead of bare commits. `scripts/check.sh` checks every release links to its tag.
- Revisiting a page after an API error (a timeout, a 500, "busy") asks the API again instead of showing the same error for up to 5 minutes. Prefetches no longer store errors, and a garbled answer is never reused as data.
- Back to the front page deep in the feed returns to the same card and stays there. Before, the list came back shorter (25 cards, without the later pages), and a position saved by pixels could not be reached or was moved by images loading, so it could jump to the top a few seconds later. The place is now saved as the card at the top of the viewport, lists come back as long as they were, and the place is held for 6 s unless the reader scrolls.
- Back to a feed no longer moves the page after the reader's place has been restored. Images above that place came back at zero height and grew as they loaded, pushing the page by hundreds of pixels at a time, sometimes after the 6 s hold had ended. Images seen before are now drawn at their size straight away.
- Going Back or navigating to another page dismisses the media viewer and releases its scroll lock, instead of leaving it over the destination page. Closing with Escape still returns focus to the image that opened it.
- A video about to autoplay no longer flashes the play icon over its poster: the icon stays hidden from the moment it is due to autoplay until it plays. It comes back if playback hasn't started within 2.5 s, for example when autoplay is refused.
- A video autoplaying muted no longer has the play button drawn over it, so it no longer looks paused. A small "Sound off · tap for sound" badge shows instead, and a click anywhere plays it with sound. Videos are muted and inline in their markup, and muted again before every autoplay, as iOS Safari requires.
- A video the reader started keeps playing when another video on the page could autoplay. Before, a muted autoplay that started at the same moment took over and stopped the reader's video.
- Autoplay goes to the topmost video in view. On phones, where a post's video and a reply's can both be on screen, whichever loaded last used to take over, so the reply's video could stop the post's. Once the playing video scrolls away, the next one in view starts.
- The Content Security Policy allows Cloudflare Web Analytics' script (`static.cloudflareinsights.com`). Cloudflare injects it when Web Analytics is on, and before, the browser blocked it on every page and logged an error.
- A cast whose posting app couldn't be looked up (Hypersnap failed, or the request's budget ran out) is looked up again on a later view. Before, it was remembered as having no app for as long as the Worker stayed warm.
- The header search box says "Search", so its placeholder no longer gets cut off on phones. Its accessible name still says what it searches.
- The Farcaster app is called Farcaster: casts posted from it say "via Farcaster", not "via Warpcast" (its account, fid 9152, still has its old name). The front page shows a thin bar of the apps today's trending casts (or those of the time range picked) were posted from, with each app's share, from each cast's provenance (the app that registered its signing key). It comes with the trending payload, so it costs no extra request, and shows once at least 20 casts have a known app.
- When the next page of the front page fails to load, it is tried again after 2, 4 and 8 seconds. Before, one failed page ended the feed for the rest of the visit.
- Channel pages show the channel's banner, picture, description, follower and member counts, link, lead and moderators, and its pinned cast above the feed, from the Farcaster app's public channel API. Member counts are now true member counts: Hypersnap's number counted distinct casters (it said /dev had 15,664 members; it has 475), so where only that number is known, as in search and the static copy, it is shown as casters. Where that API can't be reached, the page shows what Hypersnap gives.
- A short or empty filtered page no longer stops infinite scrolling while later casts remain. Lists recheck their end after a page arrives, respect retry backoff and the 1,000-cast request limit, and release their observer callbacks when unmounted. Ranked front-page batches are cached between reactive reads.
- Changing the front-page sort reorders each loaded trending page, including later pages, while keeping page boundaries stable as more arrives. Later pages retain their posting-app metadata, including when returning to the feed. A request that finishes after navigating away no longer accesses disposed feed state.
- Popovers (who liked, recasted, replies) open toward the side with more room, so the replies preview on the first card no longer slides under the sticky header. Their height fits the space available, and the card with an open popover sits above the next card instead of under it.
- Popovers (who liked, recasted, replied, and the Wallets list) have a solid background. In Chrome, a photo in the card above could show through them: their see-through, blurred background was drawn beneath images on their own compositing layer.
- Timestamps with impossible dates (30 February, 31 April), out-of-range time zone offsets or trailing text are rejected instead of being rolled over into a different date.
- The header search box suggests as you type: once typing pauses for a quarter of a second, a list of up to 4 people (picture, name and handle), 3 channels and 3 posts drops down, from the same cached search the results page uses. Arrow keys move through it and Enter opens the highlighted item; a click opens one; Enter with nothing highlighted, or the last row, opens all results; Escape closes it. The box is an ARIA combobox with a listbox of suggestions.
- Channel parent URLs and search terms retain literal percent escapes instead of being decoded again. The static API parser treats form-encoded spaces and literal plus signs the same way as the Worker.
- On touch screens, tapping a likes, recasts or replies count opens its popover. Before, the tap's focus opened it and the tap itself closed it again. On narrow screens, popovers are nudged to stay on screen.
- Escape closes a keyboard popover without reopening it on the returning focus. Preview links stay in the app, and leaving an open preview removes its outside-click listener.
- On phones, a cast with a wide image no longer makes its list wider than the screen.
- A cast whose channel URL holds a non-ASCII character where an address would start no longer crashes the page that lists it.
- On phones, profiles show the wallet activity panel and its Wallets button right after the profile header, instead of below every cast. The Wallets popover opens toward the side with more room, like the others.
- Profiles open by any name the account holds. `/u/dwr.eth` said "No such user": Hypersnap looks names up by each account's current fname only, and fid 3 is now `dwr`. Other names, ENS names among them, are now looked up by the hub's username proof, which names the account's FID.
- Casts that embed mini apps hosted on their own domains (for example a `*.vercel.app` app) are left out of every feed, search and reply list, like those on Farcaster's hosts. Hypersnap gives no embed metadata, so the Worker reads the linked page's `fc:miniapp`, `fc:frame` or `of:` meta tags: a few links per build, with each verdict remembered per URL.
- Spam is left out of every feed, search and reply list, like mini apps: casts linking to `*.vercel.app` apps, anything mentioning or linking gig-bot, and DEGEN reward or airdrop bait ("Just unlocked my DEGEN rewards", `?degen=1` links). A spam reply that has replies of its own stays as "Spam post hidden" so the thread still reads.
- Clicking play on a video while its playlist is still loading plays it, with sound, once it has loaded. Before, the click was lost and the video stayed still.
- Leaving a loading video no longer lets a late playlist or autoplay timeout access disposed state and panic the app. Autoplay timers are cleared when replaced or when the video is removed.

## [0.3.0] - 2026-10-18

### Added

- **Search**: one box in the header for casts, people and channels, with results grouped. Search hits are re-read in bulk so they carry their media and counts.
- **Who liked, recasted and replied**:
  - Hovering (or focusing) a likes or recasts count shows the ten newest people. Clicking opens the full list (up to 100, as many as Hypersnap returns).
  - Hovering a replies count shows a scrollable preview of the most engaged replies, with avatars, scores, text and image thumbnails, and a link to the full thread. On touch screens a tap opens it.
- **Wallets popover** on profiles: every verified EVM and Solana address, each with one-click copy and a "Copied" confirmation.
- **The Farcaster vote** on profiles: whether the account voted to stay or to leave when farcaster.xyz asked, or did not vote.
  - Stay votes are read onchain on Base, from Network School's opt-in NFT keyed by FID.
  - Leave votes come from farcaster.xyz's public API, since they are not recorded onchain.
  - See [docs/VOTE.md](docs/VOTE.md).
- **Instant navigation** ([docs/PERFORMANCE.md](docs/PERFORMANCE.md)):
  - A post opens from the cast data already in the feed and loads its replies behind it.
  - Hovering or touching a card prefetches its post, and cards that stay in view are prefetched in idle time, within a capped budget.
  - Infinite scroll loads the next trending page before the reader reaches the end.
  - Navigation uses View Transitions and Speculation Rules, with preconnects to media hosts.
  - Back and forward restore the scroll position.
  - CI checks a performance budget against the live site.
- **Videos autoplay muted when scrolled into view** and pause when scrolled away, one at a time. They never autoplay with reduced motion or Save-Data.
- The whole post card is clickable, while text stays selectable and links inside it still work.
- **Release process**: Semantic Versioning from the workspace `Cargo.toml`, Conventional Commits, this changelog, tagged GitHub releases, and short-lived branches merged by squash ([docs/RELEASING.md](docs/RELEASING.md)).
- The version and short commit appear in the footer, and the Worker answers `/version` (also `/api/v1/version`).
- A test of Neynar scores for known FIDs, and [docs/SCORE.md](https://github.com/ael-dev3/Allthing/blob/v0.3.0/docs/SCORE.md) on why the score is read onchain.

### Changed

- **Casts that embed mini apps or frames are filtered out server-side**: from the front page, channels, profiles, search and reply previews. They are detected by URL (`farcaster.xyz/miniapps/…`, `farcaster.xyz/~/mini-apps…`, `warpcast.com/~/frames…`, `*.neynar.app`, including links in the text), or by `fc:frame`/`fc:miniapp` metadata where an API provides it. In threads, a mini app reply is dropped, unless it has replies of its own; then it stays as "Mini app post hidden" so the thread holds together.
- Profiles keep only the wallet activity infographic in the side area.
- The header has Home, Channels and search; the footer is one line (license, source, version).
- Changing the sort reorders the list in place instead of rebuilding the page, and expanding replies keeps the scroll position.
- Neynar scores are cached for 12 hours instead of 6 (they change weekly at most).
- GitHub Pages now redirects to https://allthing.social, keeping paths and queries.

### Removed

- The "Hypersnap" backend badges on casts, the "Read from" and ingress lines of the post's provenance box, and "view on farcaster.xyz" links.
- The keyboard shortcuts box and the j/k/o/x bindings, with their selection highlight. The media viewer keeps its arrow keys and Escape.
- The About page, the intro box, "How ranking works", "Where data comes from", and the "Front page" heading.
- Hover enlargement of images.
- On profiles: the verified addresses list, the "Posts via" breakdown, the per-chain "no activity" notes, the Solana note and "How this is measured".

### Fixed

- Clicking play on a video that was autoplaying muted could leave it muted.
- Search queries with characters that Hypersnap's query parser treats as syntax (`:`, quotes, brackets) no longer fail.

## [0.2.0] - 2026-10-17

Released before tagging began; the link points to its last commit, `90ce332`.

### Added

- Allthing rebuilt in Rust:
  - a Leptos WebAssembly app;
  - a workers-rs API Worker on Cloudflare (Free plan) at https://allthing.social, with edge caching, request coalescing and upstream guards;
  - an original light design.
- Engagement shown as it is (likes, recasts, replies; no downvotes) with engagement-based ranking.
- Inline media with an accessible media viewer, a Rust HLS player, and click-to-load YouTube, Vimeo, Loom and Streamable players.
- Neynar scores beside names; profiles with registration date, spam labels from the Farcaster app's dataset, and wallet activity by chain.
- The signing app shown on each cast, and [docs/PROVENANCE.md](docs/PROVENANCE.md).
- A static GitHub Pages build of the same app (`static-host`).
- AGPL-3.0-or-later licensing with a dependency license audit.

## [0.1.0] - 2026-10-17

Released before tagging began; the link points to its last commit, `e521cd1`.

### Added

- A read-only Next.js prototype with live Hypersnap data, on GitHub Pages.

[Unreleased]: https://github.com/ael-dev3/Allthing/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/ael-dev3/Allthing/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/ael-dev3/Allthing/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ael-dev3/Allthing/releases/tag/v0.1.0

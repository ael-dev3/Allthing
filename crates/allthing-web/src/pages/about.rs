// SPDX-License-Identifier: AGPL-3.0-or-later
//! What Allthing is and how it works.

use leptos::prelude::*;

use crate::util::{PROVENANCE_DOC_URL, SOURCE_URL, set_title};

#[cfg(not(feature = "static-host"))]
const BUILT_WITH: &str = "The interface is Rust compiled to WebAssembly, and so is the Cloudflare Worker that reads and caches Farcaster data for it. Pages and media load from static files; each page costs one cached API call. Videos and images load directly from where they are hosted.";
#[cfg(feature = "static-host")]
const BUILT_WITH: &str = "The interface is Rust compiled to WebAssembly. This static copy needs no server: your browser runs the same Rust code as Allthing's Cloudflare Worker and reads Hypersnap and public RPCs directly. Videos and images load directly from where they are hosted.";

#[component]
pub fn About() -> impl IntoView {
    set_title("About");
    view! {
        <article class="prose">
            <h1 class="page-title">"About Allthing"</h1>
            <p class="lead">
                "Allthing is an independent, read-only client for Farcaster, the open social network. It is a quiet place to read: no sign-in, no posting, no feeds tuned to keep you scrolling."
            </p>
            <h2>"Honest engagement"</h2>
            <p>"Farcaster has likes, recasts and replies, and no downvotes, so Allthing shows exactly those counts. Listings rank by engagement points, likes + 2 × recasts + replies. "<em>"hot"</em>" lets newer casts compete with older popular ones; ten times the engagement buys 12.5 hours."</p>
            <h2>"People, in context"</h2>
            <p>"Next to each name is the account's Neynar user score, read onchain from Neynar's score contract on Base. Profiles show when an account registered, its followers, its wallets' activity by chain, and any spam label the main Farcaster app has published. Labels are context only: Allthing never hides anyone's casts."</p>
            <h2>"Provenance"</h2>
            <p>"Every cast is signed by a key that an app registered for the account, so Allthing can say which app a cast was posted from. All data is read through Hypersnap, a read node that mirrors Snapchain mainnet. Farcaster does not record which node a message first reached; the "<a href=PROVENANCE_DOC_URL>"provenance design note"</a>" describes how a chain could."</p>
            <h2>"Built in Rust"</h2>
            <p>{BUILT_WITH}</p>
            <h2>"Free software"</h2>
            <p>"Allthing is licensed under the GNU Affero General Public License, version 3 or later. If you run a modified version for others, you must share its source too. "<a href=SOURCE_URL>"Get the source code"</a>"."</p>
            <p class="muted">"The name nods to the Allthing of Dan Simmons' Hyperion Cantos, the Hegemony's great datasphere forum. Allthing is not affiliated with Farcaster, Neynar or Quilibrium."</p>
        </article>
    }
}

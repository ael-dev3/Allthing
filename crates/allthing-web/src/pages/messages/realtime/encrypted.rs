// SPDX-License-Identifier: AGPL-3.0-or-later
//! Encrypted conversations: one-to-one messages, end to end (`allthing_core::e2e`). The
//! database keeps ciphertext only; this browser seals what the reader
//! writes and opens what they read, with its X25519 key, which WebCrypto
//! keeps unexportable in IndexedDB (`keys::Realtime`).

use std::collections::BTreeMap;

use allthing_core::e2e::{self, MAX_TEXT, Party};
use allthing_core::stdb::{DmRow, KeyRow, hex, unhex};
use leptos::ev;
use leptos::prelude::*;

use super::inbox::BackLink;
use super::live::{Live, Phase};
use super::{Who, time_label};
use crate::pages::messages::people::People;

/// What a message reads as, once this browser has tried to open it.
pub type Opened = Result<String, String>;

/// Open every message this browser hasn't opened yet, as they come.
pub fn open_all(live: &Live, opened: RwSignal<BTreeMap<u64, Opened>>) {
    let live = live.clone();
    Effect::new(move |_| {
        let Some(me) = live.identity.get() else { return };
        if !matches!(live.phase.get(), Phase::Ready { .. }) {
            return;
        }
        let fresh: Vec<DmRow> = live
            .mirror
            .with(|m| m.dms.values().filter(|d| !opened.with_untracked(|o| o.contains_key(&d.id))).cloned().collect());
        for row in fresh {
            opened.update(|o| {
                o.insert(row.id, Err("Decrypting…".into()));
            });
            let (live, me) = (live.clone(), me.clone());
            leptos::task::spawn_local(async move {
                let text = open_row(&live, &me, &row).await;
                opened.update(|o| {
                    o.insert(row.id, text);
                });
            });
        }
    });
}

/// The messages with `fid`, and the box to write to them.
#[component]
pub fn Conversation(live: Live, people: People, fid: u64, opened: RwSignal<BTreeMap<u64, Opened>>) -> impl IntoView {
    let (mirror, identity) = (live.mirror.clone(), live.identity.clone());
    let rows = {
        let (mirror, identity) = (mirror.clone(), identity.clone());
        Memo::new(move |_| {
            let me = identity.get().unwrap_or_default();
            mirror.with(|m| {
                let mut rows: Vec<DmRow> = m
                    .dms
                    .values()
                    .filter(|d| {
                        let other = if d.sender == me { &d.recipient } else { &d.sender };
                        m.fid_of(other) == Some(fid)
                    })
                    .cloned()
                    .collect();
                rows.sort_by_key(|d| (d.sent_at, d.id));
                rows
            })
        })
    };
    let their_key = {
        let mirror = mirror.clone();
        Memo::new(move |_| mirror.with(|m| m.key_for_fid(fid).cloned().map(|k| (m.verified(&k.identity), k))))
    };
    let list = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let _ = rows.with(Vec::len);
        if let Some(list) = list.get() {
            request_animation_frame(move || list.set_scroll_top(list.scroll_height()));
        }
    });
    let messages = {
        let identity = identity.clone();
        move || {
            let me = identity.get().unwrap_or_default();
            rows.get()
                .into_iter()
                .map(|row| {
                    let mine = row.sender == me;
                    let id = row.id;
                    let text = move || opened.with(|o| o.get(&id).cloned());
                    view! {
                        <li class="dm-message" class:dm-mine=mine>
                            {move || match text() {
                                Some(Ok(text)) => view! { <p class="dm-bubble">{text}</p> }.into_any(),
                                Some(Err(why)) => view! { <p class="dm-bubble rt-sealed muted">{why}</p> }.into_any(),
                                None => view! { <p class="dm-bubble rt-sealed muted">"Decrypting…"</p> }.into_any(),
                            }}
                            <time class="dm-time" datetime=time_label(row.sent_at).0>{time_label(row.sent_at).1}</time>
                        </li>
                    }
                })
                .collect_view()
        }
    };
    let ready = {
        let phase = live.phase.clone();
        move || matches!(phase.get(), Phase::Ready { .. })
    };
    let heading = move || people.handle(fid);
    view! {
        <div class="dm-thread">
            <header class="dm-thread-head">
                <BackLink/>
                <h2 class="rt-peer">
                    {move || match their_key.get() {
                        Some((verified, _)) => view! { <Who people fid verified/> }.into_any(),
                        None => view! { <Who people fid verified=true/> }.into_any(),
                    }}
                </h2>
            </header>
            <div class="dm-scroll" node_ref=list>
                <ol class="dm-messages" aria-label=move || format!("Encrypted messages with {}", heading())>
                    {messages}
                </ol>
            </div>
            {move || match their_key.get() {
                None => view! {
                    <p class="dm-empty muted rt-nokey">
                        {move || heading()}" hasn't opened Messages on Allthing yet, so there's no key to write to them with. Once they do, you can."
                    </p>
                }
                .into_any(),
                Some((_, key)) => {
                    let live = live.clone();
                    let identity = identity.clone();
                    view! { <Show when=ready.clone()><SealedComposer live=live.clone() identity=identity.clone() key=key.clone() label=heading()/></Show> }.into_any()
                }
            }}
        </div>
    }
}

/// The text box for a conversation: what's typed is sealed here and sent
/// as ciphertext.
#[component]
fn SealedComposer(live: Live, identity: ArcRwSignal<Option<String>>, key: KeyRow, label: String) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let problem = RwSignal::new(None::<String>);
    let sending = RwSignal::new(false);
    let live = StoredValue::new(live);
    let key = StoredValue::new(key);
    let identity = RwSignal::from(identity);
    let bytes = move || text.with(|t| t.trim().len());
    let send = move || {
        let body = text.get_untracked().trim().to_owned();
        if body.is_empty() || body.len() > MAX_TEXT || sending.get_untracked() {
            return;
        }
        let Some(me) = identity.get_untracked() else { return };
        sending.set(true);
        let (live, key) = (live.get_value(), key.get_value());
        leptos::task::spawn_local(async move {
            match seal_and_send(&live, &me, &key, &body).await {
                Ok(()) => {
                    text.set(String::new());
                    problem.set(None);
                }
                Err(why) => problem.set(Some(why)),
            }
            sending.set(false);
        });
    };
    view! {
        <form class="dm-composer" on:submit=move |ev: ev::SubmitEvent| { ev.prevent_default(); send(); }>
            <label class="visually-hidden" for="rt-sealed-text">{format!("Encrypted message to {label}")}</label>
            <textarea
                id="rt-sealed-text"
                class="dm-text"
                rows="1"
                placeholder="Write an encrypted message"
                prop:value=move || text.get()
                on:input=move |ev| { text.set(event_target_value(&ev)); problem.set(None); }
                on:keydown=move |ev: ev::KeyboardEvent| {
                    if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
                        ev.prevent_default();
                        send();
                    }
                }
            ></textarea>
            <span class="dm-left" class:dm-over=move || { bytes() > MAX_TEXT }>
                {move || (bytes() > 3600).then(|| format!("{} bytes left", MAX_TEXT as i64 - bytes() as i64))}
            </span>
            <button type="submit" class="button primary dm-send" disabled=move || { let n = bytes(); n == 0 || n > MAX_TEXT || sending.get() }>
                "Send"
            </button>
            <p class="dm-problem" role="alert">{move || problem.get()}</p>
        </form>
    }
}

fn public_key(row: &KeyRow) -> Option<[u8; 32]> {
    unhex(&row.x25519_pub)?.try_into().ok()
}

/// This browser's party: its identity, current key version and key.
fn my_party(live: &Live, me: &str) -> Result<(u32, [u8; 32]), String> {
    let public = live.public_key().ok_or("This browser has no message key yet")?;
    let version = live
        .mirror
        .with_untracked(|m| m.keys.get(me).filter(|k| k.x25519_pub == hex(&public)).map(|k| k.key_version))
        .ok_or("Your key isn't published yet")?;
    Ok((version, public))
}

async fn seal_and_send(live: &Live, me: &str, theirs: &KeyRow, text: &str) -> Result<(), String> {
    let (my_version, my_public) = my_party(live, me)?;
    let their_public = public_key(theirs).ok_or("Their key is malformed")?;
    let secret = live.secret(&their_public).await?;
    let nonce = crate::keys::random::<24>().ok_or("This browser has no random numbers")?;
    let sender = Party { identity: me, key_version: my_version, public_key: &my_public };
    let recipient = Party { identity: &theirs.identity, key_version: theirs.key_version, public_key: &their_public };
    let sealed = e2e::seal(&secret, &sender, &recipient, &nonce, text).map_err(|e| e.to_string())?;
    live.call(
        "sendDm",
        &[
            theirs.identity.clone().into(),
            my_version.into(),
            theirs.key_version.into(),
            hex(&nonce).into(),
            hex(&sealed).into(),
        ],
    )
    .await
}

/// A message's text, opened with this browser's key, or why it can't be.
async fn open_row(live: &Live, me: &str, row: &DmRow) -> Opened {
    let (my_version, my_public) = my_party(live, me)?;
    let mine = row.sender == me;
    let (peer, my_side, their_side) = if mine {
        (&row.recipient, row.sender_key_version, row.recipient_key_version)
    } else {
        (&row.sender, row.recipient_key_version, row.sender_key_version)
    };
    if my_side != my_version {
        return Err("Sealed for an earlier key of this browser's: it can't be read here.".into());
    }
    let theirs =
        live.mirror.with_untracked(|m| m.keys.get(peer).cloned()).ok_or("Their key is gone: it can't be read.")?;
    if theirs.key_version != their_side {
        return Err("Sealed with an earlier key of theirs: it can't be read here.".into());
    }
    let their_public = public_key(&theirs).ok_or("Their key is malformed")?;
    let secret = live.secret(&their_public).await?;
    let me_party = Party { identity: me, key_version: my_version, public_key: &my_public };
    let them = Party { identity: peer, key_version: theirs.key_version, public_key: &their_public };
    let (sender, recipient) = if mine { (me_party, them) } else { (them, me_party) };
    let nonce = unhex(&row.nonce).ok_or("Malformed message")?;
    let ciphertext = unhex(&row.ciphertext).ok_or("Malformed message")?;
    e2e::open(&secret, &sender, &recipient, &nonce, &ciphertext)
        .map_err(|e| format!("This message can't be decrypted ({e})."))
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! The inbox: one list of the reader's conversations, Allthing's public
//! `#general` room first, then their encrypted conversations, newest first;
//! beside it on a computer, or in its place on a phone, the one open:
//! `?room=general`, `?to=<fid>`, or `?new=1` to start one.

use std::collections::BTreeMap;

use allthing_core::e2e::messages_explainer;
use allthing_core::model::UserSummary;
use allthing_core::stdb::GENERAL_ROOM;
use leptos::prelude::*;

use super::encrypted::{Conversation, Opened, open_all};
use super::live::Live;
use super::{Status, chat, time_label};
use crate::components::chips::Avatar;
use crate::components::encryption::EncryptionNote;
use crate::pages::messages::people::{People, PersonPicker};
use crate::util::{dm_href, new_message_href, room_href, use_go, use_query_param};

/// What the pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Open {
    Nothing,
    Room,
    With(u64),
    New,
}

#[component]
pub fn Inbox(live: Live, people: People) -> impl IntoView {
    let (room, to, new) = (use_query_param("room"), use_query_param("to"), use_query_param("new"));
    let open = Memo::new(move |_| {
        if let Some(fid) = to.get().and_then(|t| t.parse::<u64>().ok()).filter(|f| *f > 0) {
            Open::With(fid)
        } else if room.with(Option::is_some) {
            Open::Room
        } else if new.with(Option::is_some) {
            Open::New
        } else {
            Open::Nothing
        }
    });
    let go = use_go();
    let mirror = live.mirror.clone();
    let identity = live.identity.clone();
    let opened = RwSignal::new(BTreeMap::<u64, Opened>::new());
    open_all(&live, opened);

    // The encrypted conversations, by the other account, newest first.
    let threads = {
        let (mirror, identity) = (mirror.clone(), identity.clone());
        Memo::new(move |_| {
            let me = identity.get().unwrap_or_default();
            mirror.with(|m| {
                let mut seen = Vec::<(u64, i64)>::new();
                for t in m.threads(&me) {
                    if let Some(fid) = m.fid_of(&t.peer) {
                        match seen.iter_mut().find(|(f, _)| *f == fid) {
                            Some(entry) => entry.1 = entry.1.max(t.last_at),
                            None => seen.push((fid, t.last_at)),
                        }
                    }
                }
                seen.sort_by_key(|t| std::cmp::Reverse(t.1));
                seen
            })
        })
    };
    let room_last = {
        let mirror = mirror.clone();
        Memo::new(move |_| mirror.with(|m| m.room_messages(GENERAL_ROOM).last().map(|m| m.sent_at)))
    };
    Effect::new(move |_| people.want(threads.with(|t| t.iter().map(|(f, _)| *f).collect::<Vec<_>>())));
    Effect::new(move |_| {
        if let Open::With(fid) = open.get() {
            people.want([fid]);
        }
    });

    let rows = move || {
        let current = open.get();
        threads
            .get()
            .into_iter()
            .map(|(fid, at)| {
                let name = move || people.get(fid).map_or_else(|| format!("fid {fid}"), |u| u.name().to_owned());
                view! {
                    <li>
                        <a class="dm-row rt-thread" href=dm_href(fid) aria-current=(current == Open::With(fid)).then_some("page")>
                            {move || view! { <Avatar url=people.get(fid).and_then(|u| u.pfp_url) size=32/> }}
                            <span class="dm-row-text">
                                <span class="dm-row-name">{name}</span>
                                <span class="dm-row-last">"Encrypted"</span>
                            </span>
                            <time class="dm-row-time" datetime=time_label(at).0>{time_label(at).1}</time>
                        </a>
                    </li>
                }
            })
            .collect_view()
    };

    let picked = Callback::new(move |user: UserSummary| {
        people.add(vec![user.clone()]);
        go(&dm_href(user.fid), Default::default());
    });

    let pane = {
        let live = live.clone();
        move || {
            match open.get() {
            Open::Nothing => view! {
                <p class="dm-empty muted">"Open #general, or a conversation, or start one with New message."</p>
            }
            .into_any(),
            Open::Room => view! { <chat::Chat live=live.clone() people/> }.into_any(),
            Open::New => view! {
                <div class="dm-thread">
                    <header class="dm-thread-head">
                        <BackLink/>
                        <h2 class="rt-peer">"New encrypted message"</h2>
                    </header>
                    <div class="rt-start">
                        <p class="muted">"Write to someone by name. They need to have opened Messages on Allthing once, so their browser has a key."</p>
                        <PersonPicker people picked/>
                    </div>
                </div>
            }
            .into_any(),
            Open::With(fid) => view! { <Conversation live=live.clone() people fid opened/> }.into_any(),
        }
        }
    };

    view! {
        <div class="rt-bar">
            <EncryptionNote explainer=messages_explainer()/>
        </div>
        <Status live=live.clone()/>
        <div class="dm-box rt-box" class:dm-has-pane=move || open.get() != Open::Nothing>
            <nav class="dm-list" aria-label="Conversations">
                <a class="button rt-new" href=new_message_href()>"New message"</a>
                <ul class="dm-rows">
                    <li>
                        <a class="dm-row rt-thread" href=room_href() aria-current=move || (open.get() == Open::Room).then_some("page")>
                            <span class="rt-room-icon" aria-hidden="true">"#"</span>
                            <span class="dm-row-text">
                                <span class="dm-row-name">"#general"</span>
                                <span class="dm-row-last">"Allthing's public room"</span>
                            </span>
                            {move || room_last.get().map(|at| view! { <time class="dm-row-time" datetime=time_label(at).0>{time_label(at).1}</time> })}
                        </a>
                    </li>
                    {rows}
                </ul>
            </nav>
            <div class="dm-pane">{pane}</div>
        </div>
    }
}

/// On a phone, the way back to the list.
#[component]
pub fn BackLink() -> impl IntoView {
    view! {
        <a class="dm-back" href=crate::util::messages_href() data-noscroll="" aria-label="Back to your conversations">
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d="M10 3 5 8l5 5"/></svg>
        </a>
    }
}

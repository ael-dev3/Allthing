// SPDX-License-Identifier: AGPL-3.0-or-later
//! `#general`: Allthing's public room, live, in the inbox's pane. Its messages are public (anyone
//! signed in reads them), stored in the clear in the database's
//! `chat_message`, and never posted to Farcaster.

use std::time::Duration;

use allthing_core::stdb::{GENERAL_ROOM, MAX_CHAT_BYTES};
use leptos::ev;
use leptos::prelude::*;

use super::inbox::BackLink;
use super::live::{Live, Phase};
use super::{Clock, Who, time_label};
use crate::pages::messages::people::People;

/// While typing, the room is told at most this often (the module keeps
/// the line 6 s).
const TYPING_EVERY: Duration = Duration::from_secs(3);
/// How close to its end the list must be scrolled for a new message to
/// scroll it on.
const NEAR_END_PX: i32 = 96;

#[component]
pub fn Chat(live: Live, people: People) -> impl IntoView {
    let mirror = live.mirror.clone();
    let identity = live.identity.clone();
    let phase = live.phase.clone();
    let clock = Clock::new();
    let list = NodeRef::<leptos::html::Ol>::new();

    let messages = {
        let mirror = mirror.clone();
        Memo::new(move |_| {
            mirror.with(|m| {
                m.room_messages(GENERAL_ROOM)
                    .into_iter()
                    .filter(|message| {
                        !people.hidden(message.sender_fid) && !crate::moderation::hides_text(&message.text)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
        })
    };
    Effect::new(move |_| people.want(messages.with(|ms| ms.iter().map(|m| m.sender_fid).collect::<Vec<_>>())));
    // Scrolled to the end as messages come, unless the reader scrolled up.
    Effect::new(move |previous: Option<usize>| {
        let count = messages.with(Vec::len);
        if let Some(list) = list.get() {
            let near = list.scroll_height() - list.scroll_top() - list.client_height() < NEAR_END_PX;
            if previous.is_none() || previous == Some(0) || near {
                request_animation_frame(move || list.set_scroll_top(list.scroll_height()));
            }
        }
        count
    });
    let online = {
        let mirror = mirror.clone();
        move || mirror.with(|m| m.online())
    };
    let typing = {
        let (mirror, identity) = (mirror.clone(), identity.clone());
        move || {
            let now = clock.now_us();
            let me = identity.get().unwrap_or_default();
            let fids = mirror.with(|m| {
                m.typing_in(GENERAL_ROOM, &me, now).into_iter().filter(|fid| !people.hidden(*fid)).collect::<Vec<_>>()
            });
            match fids.as_slice() {
                [] => None,
                [one] => Some(format!("{} is typing…", people.handle(*one))),
                [one, two] => Some(format!("{} and {} are typing…", people.handle(*one), people.handle(*two))),
                more => Some(format!("{} people are typing…", more.len())),
            }
        }
    };
    let rows = {
        let (mirror, identity) = (mirror.clone(), identity.clone());
        move || {
            let me = identity.get().unwrap_or_default();
            messages
                .get()
                .into_iter()
                .map(|m| {
                    let verified = mirror.with(|mi| mi.verified(&m.sender));
                    let mine = m.sender == me;
                    view! {
                        <li class="rt-msg" class:rt-mine=mine>
                            <Who people fid=m.sender_fid verified/>
                            <time class="rt-time muted" datetime=time_label(m.sent_at).0>{time_label(m.sent_at).1}</time>
                            <p class="rt-text">{m.text}</p>
                        </li>
                    }
                })
                .collect_view()
        }
    };
    let ready = {
        let phase = phase.clone();
        move || matches!(phase.get(), Phase::Ready { .. })
    };

    view! {
        <div class="dm-thread rt-room">
            <header class="dm-thread-head">
                <BackLink/>
                <h2 class="rt-peer">"#general"</h2>
                <span class="rt-online muted">{move || format!("{} online", online())}</span>
            </header>
            <ol class="rt-list dm-scroll" node_ref=list aria-label="Messages in #general" aria-live="polite">
                {rows}
            </ol>
            <div class="rt-foot">
                <p class="rt-typing muted" aria-live="polite">{typing}</p>
                <Show when=ready>
                    <RoomComposer live=live.clone()/>
                </Show>
            </div>
        </div>
    }
}

/// The room's text box: Enter sends, Shift+Enter starts a new line.
#[component]
fn RoomComposer(live: Live) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let problem = RwSignal::new(None::<String>);
    let sending = RwSignal::new(false);
    let last_typing = StoredValue::new(0.0_f64);
    let live = StoredValue::new(live);
    let bytes = move || text.with(|t| t.trim().len());
    let send = move || {
        let body = text.get_untracked().trim().to_owned();
        if body.is_empty() || body.len() > MAX_CHAT_BYTES || sending.get_untracked() {
            return;
        }
        sending.set(true);
        let live = live.get_value();
        leptos::task::spawn_local(async move {
            match live.call("sendMessage", &[(GENERAL_ROOM as f64).into(), body.into()]).await {
                Ok(()) => {
                    text.set(String::new());
                    problem.set(None);
                    last_typing.set_value(0.0);
                }
                Err(why) => problem.set(Some(why)),
            }
            sending.set(false);
        });
    };
    let typed = move || {
        let now = js_sys::Date::now();
        if now - last_typing.get_value() > TYPING_EVERY.as_millis() as f64 {
            last_typing.set_value(now);
            let live = live.get_value();
            leptos::task::spawn_local(async move {
                let _ = live.call("setTyping", &[(GENERAL_ROOM as f64).into(), true.into()]).await;
            });
        }
    };
    view! {
        <form class="dm-composer rt-composer" on:submit=move |ev: ev::SubmitEvent| { ev.prevent_default(); send(); }>
            <label class="visually-hidden" for="rt-text">"Message #general"</label>
            <textarea
                id="rt-text"
                class="dm-text"
                rows="1"
                placeholder="Message #general"
                prop:value=move || text.get()
                on:input=move |ev| {
                    text.set(event_target_value(&ev));
                    problem.set(None);
                    typed();
                }
                on:keydown=move |ev: ev::KeyboardEvent| {
                    if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
                        ev.prevent_default();
                        send();
                    }
                }
            ></textarea>
            <span class="dm-left" class:dm-over=move || { bytes() > MAX_CHAT_BYTES }>
                {move || (bytes() > 3600).then(|| format!("{} bytes left", MAX_CHAT_BYTES as i64 - bytes() as i64))}
            </span>
            <button type="submit" class="button primary dm-send" disabled=move || { let n = bytes(); n == 0 || n > MAX_CHAT_BYTES || sending.get() }>
                "Send"
            </button>
            <p class="dm-problem" role="alert">{move || problem.get()}</p>
        </form>
    }
}

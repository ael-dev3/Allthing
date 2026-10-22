// SPDX-License-Identifier: AGPL-3.0-or-later
//! The composer's "@" suggestions, as the Farcaster app's: an "@name" being
//! typed (`allthing_core::text::mention_query`) opens a list of the
//! accounts it may mean under the caret, people the reader follows first,
//! each with its picture, name, @username and "Following" when the reader
//! follows them. Up and Down move through it, Enter or Tab picks, Escape
//! closes it; a click or a tap picks too. Picking puts "@username " in
//! place of what was typed, which posts as a mention as any "@name" does.
//! In the composer's part of the app (`composer_extras`).

use std::collections::HashSet;
use std::time::Duration;

use allthing_core::api::{SearchPayload, search_path};
use allthing_core::model::User;
use allthing_core::text::{complete_mention, mention_query};
use leptos::html::Div;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::api;
use crate::components::chips::Avatar;
use crate::components::editor;
use crate::components::writer::use_writer;

/// Typing pauses this long before accounts are looked up.
const DEBOUNCE: Duration = Duration::from_millis(200);
/// The most accounts listed.
const SHOWN: usize = 6;
/// The list's id, for the text box's `aria-controls`.
const LIST_ID: &str = "mention-list";
/// About how tall a row is, to place the list above the caret when there's
/// no room under it.
const ROW: f64 = 52.0;

/// The suggestions for the text box `text`, whose draft is `draft`.
#[component]
pub fn MentionSuggest(text: NodeRef<Div>, draft: RwSignal<String>) -> impl IntoView {
    let writer = use_writer();
    // The "@name" being typed: where its "@" is, and the name.
    let typing = RwSignal::new(None::<(usize, String)>);
    // The accounts found, and for which name.
    let found = RwSignal::new(Vec::<User>::new());
    let found_for = StoredValue::new(String::new());
    let active = RwSignal::new(0usize);
    // Where the list goes: (left, top or bottom, above).
    let place = RwSignal::new((0.0, 0.0, false));
    // The "@" whose list Escape closed: it stays closed while that name is
    // typed.
    let dismissed = StoredValue::new(None::<usize>);
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let asked = StoredValue::new(HashSet::<u64>::new());

    let follows = move |fid: u64| writer.and_then(|w| w.follows(fid)) == Some(true);
    // What is listed: people the reader follows first, then names that
    // start with what was typed, as search ranks them otherwise.
    let shown = Memo::new(move |_| {
        let name = typing.with(|t| t.as_ref().map(|(_, n)| n.clone())).unwrap_or_default();
        let mut users = found.get();
        users.sort_by_key(|u| (!follows(u.fid), !u.username.to_ascii_lowercase().starts_with(&name)));
        users.truncate(SHOWN);
        users
    });
    let open = move || typing.with(Option::is_some) && !shown.with(Vec::is_empty);

    let cancel = move || {
        timer.update_value(|t| {
            if let Some(handle) = t.take() {
                handle.clear();
            }
        });
    };
    let close = move || {
        cancel();
        typing.set(None);
    };
    // Whether the reader follows those found, asked of the node once each
    // (a public read, from the browser).
    let ask_follows = move |users: &[User]| {
        let Some(writer) = writer else { return };
        let Some(reader) = writer.reader() else { return };
        for user in users.iter().take(SHOWN) {
            if user.fid != reader
                && writer.follows(user.fid).is_none()
                && asked.try_update_value(|a| a.insert(user.fid)) == Some(true)
            {
                writer.look_up_follow(user.fid);
            }
        }
    };
    let take = move |name: String, payload: &SearchPayload| {
        if typing.with_untracked(|t| t.as_ref().map(|(_, n)| n.as_str()) != Some(name.as_str())) {
            return;
        }
        ask_follows(&payload.users);
        found.set(payload.users.clone());
        found_for.set_value(name);
        active.set(0);
    };
    let look_up = move |name: String| {
        let path = search_path(&name);
        // Asked before: at once.
        if let Some(Ok(fetched)) = api::peek::<SearchPayload>(&path) {
            take(name, &fetched.data);
            return;
        }
        // Meanwhile, those already found that still fit.
        if found_for.with_value(|f| !f.is_empty() && name.starts_with(f.as_str())) {
            found.update(|users| users.retain(|u| u.username.to_ascii_lowercase().contains(&name)));
            active.set(0);
        } else {
            found.set(Vec::new());
        }
        cancel();
        let handle = set_timeout_with_handle(
            move || {
                leptos::task::spawn_local(async move {
                    if let Ok(fetched) = api::get::<SearchPayload>(&path).await {
                        take(name, &fetched.data);
                    }
                })
            },
            DEBOUNCE,
        );
        timer.set_value(handle.ok());
    };

    // Where the caret is, under which the list goes.
    let measure = move || {
        let Some(range) =
            web_sys::window().and_then(|w| w.get_selection().ok().flatten()).and_then(|s| s.get_range_at(0).ok())
        else {
            return;
        };
        let mut rect = range.get_bounding_client_rect();
        if rect.height() == 0.0
            && let Some(root) = text.get_untracked()
        {
            rect = root.get_bounding_client_rect();
        }
        let Some(window) = web_sys::window() else { return };
        let width = window.inner_width().ok().and_then(|w| w.as_f64()).unwrap_or(390.0);
        let height = window.inner_height().ok().and_then(|h| h.as_f64()).unwrap_or(800.0);
        let list = (SHOWN as f64) * ROW + 8.0;
        let left = rect.left().min(width - 300.0 - 8.0).max(8.0);
        let above = rect.bottom() + list > height && rect.top() - list > 0.0;
        let y = if above { height - rect.top() + 4.0 } else { rect.bottom() + 4.0 };
        place.set((left, y, above));
    };

    // Read the text box: the name typed at the caret, if any.
    let check = move || {
        let Some(root) = text.get_untracked() else { return };
        let caret = editor::caret_in(&root).filter(|(from, to)| from == to);
        let now = caret.and_then(|(at, _)| mention_query(&editor::text_of(&root), at));
        let Some((start, name)) = now else {
            dismissed.set_value(None);
            close();
            return;
        };
        if dismissed.get_value() == Some(start) {
            return;
        }
        let same = typing.with_untracked(|t| t.as_ref() == Some(&(start, name.clone())));
        if same {
            return;
        }
        measure();
        typing.set(Some((start, name.clone())));
        look_up(name);
    };

    let pick = move |i: usize| {
        let Some(user) = shown.with_untracked(|s| s.get(i).cloned()) else { return };
        let Some(root) = text.get_untracked() else { return };
        let Some((start, _)) = typing.get_untracked() else { return };
        let value = editor::text_of(&root);
        let end = value.encode_utf16().count();
        let caret = editor::caret_in(&root).map_or(end, |(_, to)| to);
        let (value, at) = complete_mention(&value, start, caret, &user.username);
        close();
        editor::show(&root, &value);
        let _ = root.focus();
        editor::place(&root, at, at);
        draft.set(value);
    };

    let keydown = move |ev: web_sys::KeyboardEvent| {
        if !open() || ev.is_composing() || ev.ctrl_key() || ev.meta_key() || ev.alt_key() {
            return;
        }
        let n = shown.with_untracked(Vec::len);
        let handled = match ev.key().as_str() {
            "ArrowDown" => {
                active.update(|a| *a = (*a + 1) % n);
                true
            }
            "ArrowUp" => {
                active.update(|a| *a = (*a + n - 1) % n);
                true
            }
            "Enter" | "Tab" if !ev.shift_key() => {
                pick(active.get_untracked());
                true
            }
            // Closes the list, not the composer.
            "Escape" => {
                dismissed.set_value(typing.with_untracked(|t| t.as_ref().map(|(s, _)| *s)));
                close();
                true
            }
            _ => false,
        };
        if handled {
            ev.prevent_default();
            ev.stop_propagation();
        }
    };

    // The text box's listeners, kept while the list is, and what the box
    // tells a screen reader.
    type Listeners = (web_sys::Element, Closure<dyn Fn(web_sys::KeyboardEvent)>, Closure<dyn Fn()>, Closure<dyn Fn()>);
    let listening = StoredValue::new_local(None::<Listeners>);
    let stop = move || {
        let Some(Some((element, on_key, on_change, on_blur))) = listening.try_update_value(Option::take) else {
            return;
        };
        let _ = element.remove_event_listener_with_callback_and_bool("keydown", on_key.as_ref().unchecked_ref(), true);
        for kind in ["input", "keyup", "click"] {
            let _ = element.remove_event_listener_with_callback(kind, on_change.as_ref().unchecked_ref());
        }
        let _ = element.remove_event_listener_with_callback("blur", on_blur.as_ref().unchecked_ref());
        for name in ["aria-autocomplete", "aria-controls", "aria-activedescendant"] {
            let _ = element.remove_attribute(name);
        }
    };
    Effect::new(move |_| {
        let Some(root) = text.get() else { return };
        stop();
        let element: web_sys::Element = root.into();
        let _ = element.set_attribute("aria-autocomplete", "list");
        let on_key = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(keydown);
        let on_change = Closure::<dyn Fn()>::new(check);
        let on_blur = Closure::<dyn Fn()>::new(close);
        let _ = element.add_event_listener_with_callback_and_bool("keydown", on_key.as_ref().unchecked_ref(), true);
        for kind in ["input", "keyup", "click"] {
            let _ = element.add_event_listener_with_callback(kind, on_change.as_ref().unchecked_ref());
        }
        let _ = element.add_event_listener_with_callback("blur", on_blur.as_ref().unchecked_ref());
        listening.set_value(Some((element, on_key, on_change, on_blur)));
        // Typed before this part of the app came.
        check();
    });
    on_cleanup(stop);
    Effect::new(move |_| {
        let Some(root) = text.get() else { return };
        let element: web_sys::Element = root.into();
        if open() {
            let _ = element.set_attribute("aria-controls", LIST_ID);
            let _ = element.set_attribute("aria-activedescendant", &format!("mention-{}", active.get()));
        } else {
            let _ = element.remove_attribute("aria-controls");
            let _ = element.remove_attribute("aria-activedescendant");
        }
    });
    on_cleanup(cancel);

    move || {
        open().then(|| {
            let (left, y, above) = place.get();
            let style = if above {
                format!("left: {left}px; bottom: {y}px")
            } else {
                format!("left: {left}px; top: {y}px")
            };
            view! {
                <ul id=LIST_ID class="mention-list" role="listbox" aria-label="People to mention" style=style>
                    {shown.get().into_iter().enumerate().map(|(i, user)| {
                        let fid = user.fid;
                        view! {
                            <li
                                id=format!("mention-{i}")
                                class="mention-option"
                                class:active=move || active.get() == i
                                role="option"
                                aria-selected=move || if active.get() == i { "true" } else { "false" }
                                // The text box keeps the focus, and its caret.
                                on:pointerdown=|ev| ev.prevent_default()
                                on:mousedown=|ev| ev.prevent_default()
                                on:mousemove=move |_| active.set(i)
                                on:click=move |_| pick(i)
                            >
                                <Avatar url=user.pfp_url.clone() size=36/>
                                <span class="mention-names">
                                    <span class="mention-name">{user.name().to_owned()}</span>
                                    <span class="mention-handle">"@"{user.username.clone()}</span>
                                </span>
                                {move || follows(fid).then(|| view! { <span class="mention-following">"Following"</span> })}
                            </li>
                        }
                    }).collect_view()}
                </ul>
            }
        })
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! Who the reader's messages are with. The database names accounts by fid
//! only, so their names and pictures come from Hypersnap's
//! `user/bulk`, which answers any origin, read by this browser for the fids
//! it hasn't seen this visit (100 at a time). And the picker that finds a
//! person to write to, by name, with the site's search.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use allthing_core::api::{SearchPayload, search_path};
use allthing_core::message::NODE;
use allthing_core::model::UserSummary;
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::api;
use crate::components::chips::Avatar;

/// Accounts read at a time.
const BULK: usize = 100;
/// Typing pauses this long before the picker looks names up.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// People the picker offers.
const OFFERED: usize = 6;

thread_local! {
    /// The accounts read this visit, and those being read.
    static ASKED: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

/// The people the page knows, by fid.
#[derive(Clone, Copy)]
pub struct People {
    known: RwSignal<HashMap<u64, UserSummary>>,
}

impl People {
    pub fn new(me: Option<UserSummary>) -> People {
        let people = People { known: RwSignal::new(HashMap::new()) };
        if let Some(me) = me {
            people.add(vec![me]);
        }
        people
    }

    /// The account `fid`, once it is known.
    pub fn get(self, fid: u64) -> Option<UserSummary> {
        self.known.with(|known| known.get(&fid).cloned())
    }

    /// What to call `fid`: "@name", or its fid until it is known.
    pub fn handle(self, fid: u64) -> String {
        self.get(fid).map_or_else(|| format!("fid {fid}"), |u| format!("@{}", u.username))
    }

    /// FID matching works before Hypersnap supplies an account's name.
    /// Resolving a name adds explicit @mention matching as well.
    pub fn hidden(self, fid: u64) -> bool {
        let user = self.get(fid).unwrap_or_else(|| UserSummary {
            fid,
            username: String::new(),
            display_name: String::new(),
            pfp_url: None,
        });
        crate::moderation::hides_user(&user)
    }

    pub fn add(self, users: Vec<UserSummary>) {
        if users.is_empty() {
            return;
        }
        ASKED.with(|asked| asked.borrow_mut().extend(users.iter().map(|u| u.fid)));
        self.known.update(|known| known.extend(users.into_iter().map(|u| (u.fid, u))));
    }

    /// Read the accounts among `fids` not read yet this visit.
    pub fn want(self, fids: impl IntoIterator<Item = u64>) {
        let wanted: Vec<u64> = ASKED.with(|asked| {
            let mut asked = asked.borrow_mut();
            let mut wanted: Vec<u64> = fids.into_iter().filter(|fid| *fid > 0 && asked.insert(*fid)).collect();
            wanted.sort_unstable();
            wanted.dedup();
            wanted
        });
        for chunk in wanted.chunks(BULK) {
            let chunk = chunk.to_vec();
            spawn_local(async move {
                match bulk(&chunk).await {
                    Some(users) => self.add(users),
                    // Asked for again next time.
                    None => ASKED.with(|asked| {
                        let mut asked = asked.borrow_mut();
                        for fid in &chunk {
                            asked.remove(fid);
                        }
                    }),
                }
            });
        }
    }
}

/// `user/bulk`'s answer: only what a row and a thread show of each account,
/// so the rest of its (large) user objects isn't parsed.
#[derive(serde::Deserialize)]
struct Bulk {
    #[serde(default)]
    users: Vec<BulkUser>,
}

#[derive(serde::Deserialize)]
struct BulkUser {
    fid: u64,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    pfp_url: Option<String>,
}

impl From<BulkUser> for UserSummary {
    fn from(user: BulkUser) -> UserSummary {
        let fid = user.fid;
        UserSummary {
            fid,
            username: user.username.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| format!("fid:{fid}")),
            display_name: user.display_name.unwrap_or_default().trim().to_owned(),
            pfp_url: user.pfp_url.filter(|url| !url.trim().is_empty()),
        }
    }
}

/// Hypersnap's accounts for `fids`.
async fn bulk(fids: &[u64]) -> Option<Vec<UserSummary>> {
    let list = fids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    let url = format!("{NODE}/v2/farcaster/user/bulk?fids={list}");
    let window = web_sys::window()?;
    let response = JsFuture::from(window.fetch_with_str(&url)).await.ok()?;
    let response: web_sys::Response = response.dyn_into().ok()?;
    if !response.ok() {
        return None;
    }
    let text = JsFuture::from(response.text().ok()?).await.ok()?.as_string()?;
    let found: Bulk = serde_json::from_str(&text).ok()?;
    Some(found.users.into_iter().map(UserSummary::from).collect())
}

/// Find the person to write to by their name or username: suggestions from
/// the site's search as the reader types, a list box the arrow keys move
/// through, and Enter or a click to pick. `picked` hears the one picked.
#[component]
pub fn PersonPicker(people: People, picked: Callback<UserSummary>) -> impl IntoView {
    let typed = RwSignal::new(String::new());
    let found = RwSignal::new(Vec::<UserSummary>::new());
    let visible = Memo::new(move |_| {
        found.get().into_iter().filter(|user| !crate::moderation::hides_user(user)).collect::<Vec<_>>()
    });
    let active = RwSignal::new(None::<usize>);
    let open = RwSignal::new(false);
    let looking = RwSignal::new(false);
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let revision = StoredValue::new(0u64);
    let look_up = move |words: String| {
        timer.update_value(|t| {
            if let Some(handle) = t.take() {
                handle.clear();
            }
        });
        revision.update_value(|r| *r += 1);
        let words = words.trim().trim_start_matches('@').to_owned();
        if words.chars().count() < 2 {
            found.set(Vec::new());
            open.set(false);
            looking.set(false);
            return;
        }
        let mine = revision.get_value();
        looking.set(true);
        let handle = set_timeout_with_handle(
            move || {
                spawn_local(async move {
                    let result = api::get::<SearchPayload>(&search_path(&words)).await;
                    if revision.get_value() != mine {
                        return;
                    }
                    looking.set(false);
                    let users: Vec<UserSummary> = result
                        .map(|fetched| {
                            fetched
                                .data
                                .users
                                .iter()
                                .take(OFFERED)
                                .map(|u| UserSummary {
                                    fid: u.fid,
                                    username: u.username.clone(),
                                    display_name: u.display_name.clone(),
                                    pfp_url: u.pfp_url.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    people.add(users.clone());
                    active.set(None);
                    open.set(true);
                    found.set(users);
                });
            },
            DEBOUNCE,
        )
        .ok();
        timer.set_value(handle);
    };
    on_cleanup(move || {
        if let Some(Some(handle)) = timer.try_get_value() {
            handle.clear();
        }
    });
    let pick = move |user: UserSummary| {
        if crate::moderation::hides_user(&user) {
            return;
        }
        open.set(false);
        active.set(None);
        picked.run(user);
    };
    let keydown = move |ev: ev::KeyboardEvent| {
        let count = visible.with_untracked(Vec::len);
        match ev.key().as_str() {
            "ArrowDown" if count > 0 => {
                ev.prevent_default();
                open.set(true);
                active.update(|a| *a = Some(a.map_or(0, |i| (i + 1) % count)));
            }
            "ArrowUp" if count > 0 => {
                ev.prevent_default();
                active.update(|a| *a = Some(a.map_or(count - 1, |i| (i + count - 1) % count)));
            }
            "Enter" => {
                ev.prevent_default();
                let chosen = visible.with_untracked(|f| {
                    let typed = typed.get_untracked();
                    let wanted = typed.trim().trim_start_matches('@').to_lowercase();
                    match active.get_untracked() {
                        Some(i) => f.get(i).cloned(),
                        // Enter on a name typed in full picks that account.
                        None => f
                            .iter()
                            .find(|u| u.username.to_lowercase() == wanted)
                            .cloned()
                            .or_else(|| (f.len() == 1).then(|| f[0].clone())),
                    }
                });
                if let Some(user) = chosen {
                    pick(user);
                }
            }
            "Escape" if open.get_untracked() => {
                ev.prevent_default();
                ev.stop_propagation();
                open.set(false);
                active.set(None);
            }
            _ => {}
        }
    };
    let shown = move || open.get() && !visible.with(Vec::is_empty);
    view! {
        <div class="dm-picker">
            <label class="dm-picker-label" for="dm-picker-input">"To"</label>
            <input
                id="dm-picker-input"
                class="dm-picker-input"
                type="text"
                role="combobox"
                autocomplete="off"
                spellcheck="false"
                placeholder="Name or @username"
                aria-autocomplete="list"
                aria-controls="dm-picker-list"
                aria-expanded=move || shown().to_string()
                aria-activedescendant=move || active.get().filter(|i| shown() && *i < visible.with(Vec::len)).map(|i| format!("dm-person-{i}"))
                prop:value=move || typed.get()
                on:input=move |ev| {
                    let value = event_target_value(&ev);
                    typed.set(value.clone());
                    look_up(value);
                }
                on:keydown=keydown
                on:blur=move |ev: ev::FocusEvent| {
                    // A pick in the list keeps it open; leaving the box for
                    // anything else closes it.
                    let inside = ev
                        .related_target()
                        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                        .and_then(|el| el.closest(".dm-picker").ok().flatten())
                        .is_some();
                    if !inside {
                        open.set(false);
                    }
                }
            />
            <ul id="dm-picker-list" class="dm-picker-list" role="listbox" aria-label="People" hidden=move || !shown()>
                {move || {
                    visible
                        .get()
                        .into_iter()
                        .enumerate()
                        .map(|(i, user)| {
                            let chosen = user.clone();
                            view! {
                                <li
                                    id=format!("dm-person-{i}")
                                    class="dm-person"
                                    role="option"
                                    aria-selected=move || (active.get() == Some(i)).to_string()
                                    on:mousedown=move |ev| ev.prevent_default()
                                    on:click=move |_| pick(chosen.clone())
                                >
                                    <Avatar url=user.pfp_url.clone() size=32/>
                                    <span class="dm-person-text">
                                        <strong>{user.name().to_owned()}</strong>
                                        <span class="muted">"@"{user.username.clone()}</span>
                                    </span>
                                </li>
                            }
                        })
                        .collect_view()
                }}
            </ul>
            <p class="dm-picker-status muted" role="status">
                {move || {
                    if looking.get() {
                        "Looking…".to_owned()
                    } else if open.get() && visible.with(Vec::is_empty) && !typed.with(|t| t.trim().is_empty()) {
                        "No one by that name.".to_owned()
                    } else {
                        String::new()
                    }
                }}
            </p>
        </div>
    }
}

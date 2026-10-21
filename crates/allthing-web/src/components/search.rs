// SPDX-License-Identifier: AGPL-3.0-or-later
//! The header's search box: one query for casts, people and channels, with
//! suggestions as the reader types.

use std::time::Duration;

use allthing_core::api::{SearchPayload, cast_path, channel_path, search_path, user_path};
use allthing_core::text::truncate_title;
use leptos::ev;
use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::api;
use crate::components::chips::Avatar;
use crate::components::text::ExcerptText;
use crate::util::{cast_href, channel_href, href, use_go, use_query_param, user_href, with_query};

/// Typing pauses this long before suggestions are looked up.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// Suggestions of each kind.
const PEOPLE: usize = 4;
const CHANNELS: usize = 3;
const POSTS: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Person,
    Channel,
    Post,
    All,
}

impl Kind {
    fn group(self) -> &'static str {
        match self {
            Kind::Person => "People",
            Kind::Channel => "Channels",
            Kind::Post => "Posts",
            Kind::All => "",
        }
    }
}

/// One suggestion: where it leads and what it shows.
#[derive(Clone)]
struct Suggestion {
    kind: Kind,
    href: String,
    /// The API path of what it opens, fetched ahead when it's highlighted.
    data: Option<String>,
    image: Option<String>,
    title: String,
    detail: String,
}

fn suggestions(query: &str, payload: &SearchPayload) -> Vec<Suggestion> {
    let people = payload.users.iter().take(PEOPLE).map(|u| Suggestion {
        kind: Kind::Person,
        href: user_href(&u.username),
        data: Some(user_path(&u.username)),
        image: u.pfp_url.clone(),
        title: u.name().to_owned(),
        detail: format!("@{}", u.username),
    });
    let channels = payload.channels.iter().take(CHANNELS).map(|c| Suggestion {
        kind: Kind::Channel,
        href: channel_href(&c.id),
        data: Some(channel_path(&c.id)),
        image: c.image_url.clone(),
        title: c.name.clone(),
        detail: format!("/{}", c.id),
    });
    let posts = payload.casts.iter().filter(|c| !c.hidden()).take(POSTS).map(|c| {
        let excerpt = truncate_title(&c.text, 90);
        Suggestion {
            kind: Kind::Post,
            href: cast_href(&c.hash),
            data: Some(cast_path(&c.hash)),
            image: c.author.pfp_url.clone(),
            title: if excerpt.is_empty() { "A cast with media".into() } else { excerpt },
            detail: format!("@{}", c.author.username),
        }
    });
    let all = Suggestion {
        kind: Kind::All,
        href: with_query(&href("/search"), &[("q", Some(query.to_owned()))]),
        data: None,
        image: None,
        title: format!("See all results for “{query}”"),
        detail: String::new(),
    };
    people.chain(channels).chain(posts).chain(std::iter::once(all)).collect()
}

#[component]
pub fn SearchBox() -> impl IntoView {
    let query = use_query_param("q");
    let location = use_location();
    let input = NodeRef::<leptos::html::Input>::new();
    let navigate = use_go();
    // Show the current query while on the results page.
    let current =
        move || location.pathname.get().ends_with("/search").then(|| query.get()).flatten().unwrap_or_default();

    // Suggestions for what is typed, which one is highlighted, and whether
    // the list shows.
    let found = RwSignal::new(Vec::<Suggestion>::new());
    // The query `found` answers.
    let found_for = StoredValue::new(String::new());
    let active = RwSignal::new(None::<usize>);
    let open = RwSignal::new(false);
    let timer = StoredValue::new(None::<TimeoutHandle>);
    // Closing or editing invalidates results that are already in flight,
    // including an earlier lookup for the same words.
    let revision = StoredValue::new(0u64);
    let cancel = move || {
        timer.update_value(|t| {
            if let Some(handle) = t.take() {
                handle.clear();
            }
        });
    };
    // Closing also drops a look-up still waiting for typing to pause.
    let close = move || {
        cancel();
        revision.update_value(|r| *r = r.wrapping_add(1));
        open.set(false);
        active.set(None);
    };
    // Any navigation closes the list.
    Effect::new(move |_| {
        location.pathname.track();
        location.search.track();
        close();
    });
    let typed = move || input.get_untracked().map(|i| i.value()).unwrap_or_default();
    let go = {
        let navigate = navigate.clone();
        move |href: &str| {
            close();
            if let Some(i) = input.get_untracked() {
                let _ = i.blur();
            }
            navigate(href, Default::default());
        }
    };

    let look_up = move |q: String, requested: u64| {
        wasm_bindgen_futures::spawn_local(async move {
            let result = api::get::<SearchPayload>(&search_path(&q)).await;
            // Only for what is still typed, while the reader is still here.
            if revision.try_get_value() != Some(requested) || typed().trim() != q {
                return;
            }
            if let Ok(fetched) = result {
                found.set(suggestions(&q, &fetched.data));
                found_for.set_value(q);
                active.set(None);
                let focused = input.get_untracked().is_some_and(|i| {
                    let element: web_sys::Element = i.into();
                    document().active_element() == Some(element)
                });
                open.set(focused);
            }
        });
    };
    let schedule = move |q: String| {
        // Previous suggestions stop being actionable as soon as the query
        // changes, not only after the next response arrives.
        close();
        if q.chars().count() < 2 {
            found.set(Vec::new());
            return;
        }
        let requested = revision.get_value();
        timer.set_value(set_timeout_with_handle(move || look_up(q, requested), DEBOUNCE).ok());
    };
    let on_input = move |ev: ev::Event| schedule(event_target_value(&ev).trim().to_owned());
    on_cleanup(move || {
        if let Some(Some(handle)) = timer.try_get_value() {
            handle.clear();
        }
    });

    let on_keydown = {
        let go = go.clone();
        move |ev: ev::KeyboardEvent| {
            let n = if found_for.with_value(|q| *q == typed().trim()) { found.with_untracked(Vec::len) } else { 0 };
            match ev.key().as_str() {
                "ArrowDown" if n > 0 => {
                    ev.prevent_default();
                    open.set(true);
                    active.update(|a| *a = Some(a.map_or(0, |i| (i + 1) % n)));
                }
                "ArrowUp" if n > 0 => {
                    ev.prevent_default();
                    open.set(true);
                    active.update(|a| *a = Some(a.map_or(n - 1, |i| (i + n - 1) % n)));
                }
                "Escape" => {
                    ev.prevent_default();
                    close();
                }
                "Enter" if open.get_untracked() => {
                    let chosen = active.get_untracked().and_then(|i| found.with_untracked(|f| f.get(i).cloned()));
                    if let Some(suggestion) = chosen {
                        ev.prevent_default();
                        go(&suggestion.href);
                    }
                }
                _ => {}
            }
        }
    };
    // What is highlighted opens at once: fetch its data ahead.
    Effect::new(move |_| {
        if let Some(path) = active.get().and_then(|i| found.with_untracked(|f| f.get(i).and_then(|s| s.data.clone()))) {
            api::prefetch(&path);
        }
    });

    let submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let q = typed();
        let q = q.trim();
        if !q.is_empty() {
            close();
            navigate(&with_query(&href("/search"), &[("q", Some(q.to_owned()))]), Default::default());
        }
    };

    let list = move || {
        let mut groups: Vec<(Kind, Vec<(usize, Suggestion)>)> = Vec::new();
        for (i, suggestion) in found.get().into_iter().enumerate() {
            match groups.last_mut() {
                Some((kind, members)) if *kind == suggestion.kind => members.push((i, suggestion)),
                _ => groups.push((suggestion.kind, vec![(i, suggestion)])),
            }
        }
        groups
            .into_iter()
            .map(|(kind, members)| {
                let options = members
                    .into_iter()
                    .map(|(i, s)| {
                        let go = go.clone();
                        let target = s.href.clone();
                        view! {
                            <div
                                role="option"
                                id=format!("suggestion-{i}")
                                class="suggestion"
                                class:all=kind == Kind::All
                                class:active=move || active.get() == Some(i)
                                aria-selected=move || (active.get() == Some(i)).to_string()
                                // Keep focus in the box, so the list stays open until chosen.
                                on:mousedown=|ev| ev.prevent_default()
                                // Only a moving pointer highlights: the list can open
                                // under a pointer at rest, which must not pick an item.
                                on:mousemove=move |_| {
                                    if active.get_untracked() != Some(i) {
                                        active.set(Some(i));
                                    }
                                }
                                on:click=move |_| go(&target)
                            >
                                {(kind != Kind::All).then(|| view! { <Avatar url=s.image.clone() size=28/> })}
                                <span class="suggestion-text">
                                    <span class="suggestion-title">
                                        {if kind == Kind::Post {
                                            view! { <ExcerptText text=s.title.clone()/> }.into_any()
                                        } else {
                                            s.title.clone().into_any()
                                        }}
                                    </span>
                                    {(!s.detail.is_empty()).then(|| view! { <span class="suggestion-detail">{s.detail.clone()}</span> })}
                                </span>
                            </div>
                        }
                    })
                    .collect_view();
                if kind == Kind::All {
                    options.into_any()
                } else {
                    view! {
                        <div role="group" aria-label=kind.group()>
                            <div class="suggestion-group" aria-hidden="true">{kind.group()}</div>
                            {options}
                        </div>
                    }
                    .into_any()
                }
            })
            .collect_view()
    };

    view! {
        <form class="search-box" role="search" action=href("/search") method="get" on:submit=submit>
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5l3 3"/></svg>
            <input
                node_ref=input
                type="search"
                name="q"
                placeholder="Search"
                aria-label="Search casts, people and channels"
                role="combobox"
                aria-autocomplete="list"
                aria-controls="search-suggestions"
                aria-expanded=move || (open.get() && !found.with(Vec::is_empty)).to_string()
                aria-activedescendant=move || active.get().filter(|_| open.get()).map(|i| format!("suggestion-{i}"))
                autocomplete="off"
                enterkeyhint="search"
                maxlength="100"
                prop:value=current
                on:input=on_input
                on:keydown=on_keydown
                on:focus=move |_| {
                    // Back in the box: the suggestions show again if they
                    // still answer what is typed.
                    if !found.with_untracked(Vec::is_empty) && found_for.with_value(|q| *q == typed().trim()) {
                        open.set(true);
                    } else {
                        schedule(typed().trim().to_owned());
                    }
                }
                on:blur=move |_| close()
            />
            <div
                id="search-suggestions"
                class="search-suggest"
                role="listbox"
                aria-label="Suggestions"
                hidden=move || !open.get() || found.with(Vec::is_empty)
            >
                {list}
            </div>
        </form>
    }
}

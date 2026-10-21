// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Messages page's own code, a part of its own (`crate::split`): the
//! reader's key, kept or pasted; their conversations, newest first, with an
//! unread dot; a conversation's messages, with older ones a page at a time;
//! the composer; and a new message to someone found by name. Where nothing
//! reads the reader's Direct Casts from here, it says why, links to the
//! Farcaster app's inbox, and still sends a message (`transport::Backend`).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use allthing_core::dc::{
    self, API_KEYS_URL, Call, Category, Conversation, ConversationList, DEVELOPER_TOOLS_URL, Group, GroupList,
    INBOX_URL, MAX_CHARS, Message, MessageList, OneConversation, Page, Send, Sent, Thread,
};
use allthing_core::model::UserSummary;
use allthing_core::text::truncate_title;
use allthing_core::time::iso_utc;
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::wasm_split::{SplitLoaderError, wasm_split};
use leptos_router::NavigateOptions;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

use super::Waiting;
use super::people::{People, PersonPicker};
use super::transport::{self, Backend};
use crate::components::chips::Avatar;
use crate::components::dc_key::use_dc_key;
use crate::components::signin::use_session;
use crate::keys;
use crate::util::{
    conversation_href, document, group_href, messages_href, new_message_href, now, use_go, use_query_param, user_href,
};

/// How often an open page asks for what's new, while it's in view.
const POLL: Duration = Duration::from_secs(20);
/// How close to its end a thread must be scrolled for a new message to
/// scroll it on.
const NEAR_END_PX: i32 = 96;
/// The longest last message a conversation's row shows.
const PREVIEW_CHARS: usize = 90;
/// Conversations whose seen time is kept, the most recent.
const SEEN_KEPT: usize = 300;

/// The page: code fetched the first time it's needed (`crate::split`).
#[wasm_split(messages, wasm_split_path = ::leptos::wasm_split, fallible, preload(preload_messages))]
pub fn inbox() -> Result<AnyView, SplitLoaderError> {
    Ok(view! { <Tabs/> }.into_any())
}

/// The page's tabs: Direct Casts, and Chat and Encrypted on SpacetimeDB,
/// whose code (`super::realtime`) is fetched only when one is opened.
#[component]
fn Tabs() -> impl IntoView {
    use super::realtime::{self, Tab};
    use crate::util::messages_tab_href;
    let asked = use_query_param("tab");
    let tab = Memo::new(move |_| match asked.get().as_deref() {
        Some("chat") => Some(Tab::Chat),
        Some("encrypted") => Some(Tab::Encrypted),
        _ => None,
    });
    let link = move |label: &'static str, which: Option<Tab>, query: Option<&'static str>| {
        view! {
            <a
                href=messages_tab_href(query)
                data-noscroll=""
                class:selected=move || tab.get() == which
                aria-current=move || (tab.get() == which).then_some("page")
            >
                {label}
            </a>
        }
    };
    view! {
        <nav class="tabs dm-tabs" aria-label="Messages">
            {link("Direct Casts", None, None)}
            {link("Chat", Some(Tab::Chat), Some("chat"))}
            {link("Encrypted", Some(Tab::Encrypted), Some("encrypted"))}
        </nav>
        {move || match tab.get() {
            None => view! { <Inbox/> }.into_any(),
            Some(which) => crate::split::lazy_page(
                || view! { <Waiting text="Loading messages…"/> },
                realtime::realtime(which),
            )
            .into_any(),
        }}
    }
}

/// Whose key this browser keeps, if any.
#[derive(Clone, Debug, PartialEq)]
enum Kept {
    SignedOut,
    None(u64),
    Key(u64, String),
}

#[component]
fn Inbox() -> impl IntoView {
    let session = use_session();
    let dc = use_dc_key();
    let kept = LocalResource::new(move || {
        let fid = session.user.with(|u| u.as_ref().map(|u| u.fid));
        let _ = dc.version();
        async move {
            match fid {
                None => Kept::SignedOut,
                Some(fid) => match keys::dc_key(fid).await {
                    Ok(Some(key)) => Kept::Key(fid, key),
                    _ => Kept::None(fid),
                },
            }
        }
    });
    let kept = Memo::new(move |_| kept.get());
    move || match kept.get() {
        None => view! { <Waiting text="Loading your messages…"/> }.into_any(),
        Some(Kept::SignedOut) => view! { <SignedOut/> }.into_any(),
        Some(Kept::None(fid)) => view! { <KeySetup fid refused=None/> }.into_any(),
        Some(Kept::Key(fid, key)) => view! { <Connected fid key/> }.into_any(),
    }
}

/// Signed in, with a key: whatever reads the reader's Direct Casts.
#[component]
fn Connected(fid: u64, key: String) -> impl IntoView {
    let key = StoredValue::new(key);
    let found = LocalResource::new(move || {
        let key = key.get_value();
        async move { transport::detect(&key).await }
    });
    move || match found.get() {
        None => view! { <Waiting text="Connecting to Farcaster…"/> }.into_any(),
        Some(Err(failure)) => view! { <KeySetup fid refused=Some(failure.message(None))/> }.into_any(),
        Some(Ok(Backend::None)) => view! { <SendOnly fid key/> }.into_any(),
        Some(Ok(backend)) => view! { <Conversations fid key backend/> }.into_any(),
    }
}

#[component]
fn SignedOut() -> impl IntoView {
    let session = use_session();
    view! {
        <section class="dm-panel" aria-labelledby="dm-signed-out">
            <h2 id="dm-signed-out">"Your Direct Casts are in the Farcaster app"</h2>
            <p>"Sign in to read and send them here too, with a Farcaster API key you make for Allthing."</p>
            <div class="dm-actions">
                <button type="button" class="button primary" on:click=move |_| session.sign_in()>"Sign in"</button>
                <a class="button" href=INBOX_URL target="_blank" rel="noopener">"Open your inbox in Farcaster"</a>
            </div>
        </section>
    }
}

/// How to make a key, and the field to paste it in. `refused` says why the
/// key kept was turned away, when it was.
#[component]
fn KeySetup(fid: u64, refused: Option<String>) -> impl IntoView {
    let dc = use_dc_key();
    let typed = RwSignal::new(String::new());
    let problem = RwSignal::new(refused);
    let saving = RwSignal::new(false);
    let relay = LocalResource::new(transport::relay_enabled);
    let save = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        match dc::check_key(&typed.get_untracked()) {
            Err(why) => problem.set(Some(why.message().to_owned())),
            Ok(key) => {
                saving.set(true);
                spawn_local(async move {
                    match keys::keep_dc_key(fid, &key).await {
                        Ok(()) => dc.changed(),
                        Err(_) => {
                            problem.set(Some("This browser couldn't keep the key.".into()));
                            saving.set(false);
                        }
                    }
                });
            }
        }
    };
    view! {
        <section class="dm-panel dm-setup" aria-labelledby="dm-setup-title">
            <h2 id="dm-setup-title">"Read and send your Direct Casts here"</h2>
            <p>"Allthing reaches your Direct Casts with a Farcaster API key you make for it. It takes a minute:"</p>
            <ol class="dm-steps">
                <li>
                    "Open "
                    <a href=API_KEYS_URL target="_blank" rel="noopener">"farcaster.xyz/~/developers/api-keys"</a>
                    ". If you don't see it, go to "
                    <a href=DEVELOPER_TOOLS_URL target="_blank" rel="noopener">"Settings → Developer tools"</a>
                    ", turn on “Enable developer tools”, then open Developers → API Keys."
                </li>
                <li>"Choose “Create an API Key”. Give it the description “Allthing”, and an expiry: a month is safer than the year it offers."</li>
                <li>"Copy the key, which Farcaster shows only once, and paste it here."</li>
            </ol>
            <form class="dm-key-form" on:submit=save>
                <label for="dm-key">"Your Farcaster API key"</label>
                <div class="dm-key-row">
                    <input
                        id="dm-key"
                        type="password"
                        autocomplete="off"
                        spellcheck="false"
                        placeholder="wc_secret_…"
                        aria-describedby="dm-key-note"
                        aria-invalid=move || problem.with(Option::is_some).to_string()
                        prop:value=move || typed.get()
                        on:input=move |ev| {
                            typed.set(event_target_value(&ev));
                            problem.set(None);
                        }
                    />
                    <button type="submit" class="button primary" disabled=move || saving.get()>"Keep key"</button>
                </div>
                <p class="dm-problem" role="alert">{move || problem.get()}</p>
            </form>
            <p id="dm-key-note" class="dm-note">
                "It can read and send your Direct Casts, so Allthing keeps it in this browser only. "
                {move || {
                    if relay.get() == Some(true) {
                        "It goes from here to Farcaster's API or, where that API won't answer this page, through Allthing's server, which passes it on with each request and keeps nothing. "
                    } else {
                        "It goes from here to Farcaster's API, and nowhere else. "
                    }
                }}
                "Revoke it whenever you like on the same Farcaster page. Forget key here, or signing out, deletes it from this browser."
            </p>
            <p><a class="dm-inbox-link" href=INBOX_URL target="_blank" rel="noopener">"Open your inbox in Farcaster"</a></p>
        </section>
    }
}

/// The line under the box: where the key is kept, and Forget key.
#[component]
fn KeyLine(fid: u64) -> impl IntoView {
    let dc = use_dc_key();
    view! {
        <p class="dm-key-line muted">
            "Your Farcaster API key is kept in this browser. "
            <button type="button" class="link-button dm-forget" on:click=move |_| dc.forget(fid)>"Forget key"</button>
        </p>
    }
}

/// Nothing reads the reader's Direct Casts from here: why, the inbox in the
/// Farcaster app, and a new message all the same.
#[component]
fn SendOnly(fid: u64, key: StoredValue<String>) -> impl IntoView {
    let session = use_session();
    let people = People::new(session.user.get_untracked());
    view! {
        <section class="dm-panel dm-send-only" aria-labelledby="dm-send-only-title">
            <h2 id="dm-send-only-title">"Reading your Direct Casts in Allthing needs Farcaster's API to allow it"</h2>
            <p>
                "Farcaster's API lets a browser read Direct Casts only on Farcaster's own sites, so allthing.social can't show yours. "
                "Read them in the Farcaster app; you can still send one from here."
            </p>
            <a class="button" href=INBOX_URL target="_blank" rel="noopener">"Open your inbox in Farcaster"</a>
        </section>
        <div class="dm-box dm-box-single">
            <div class="dm-pane">
                <NewMessage mode=Mode::SendOnly key people/>
            </div>
        </div>
        <KeyLine fid/>
    }
}

/// What the reader has seen of each conversation here (Unix seconds), kept
/// in this browser's localStorage, so a conversation with a newer message
/// from someone else shows a dot. Conversations not seen yet count from
/// when the reader first opened Messages here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Seen {
    since: i64,
    #[serde(default)]
    at: HashMap<String, i64>,
}

impl Seen {
    fn storage_key(fid: u64) -> String {
        format!("allthing.dc-seen.{fid}")
    }

    fn load(fid: u64) -> Seen {
        let storage = window().local_storage().ok().flatten();
        let kept = storage.as_ref().and_then(|s| s.get_item(&Seen::storage_key(fid)).ok().flatten());
        match kept.and_then(|text| serde_json::from_str::<Seen>(&text).ok()) {
            Some(seen) => seen,
            None => {
                let seen = Seen { since: now(), at: HashMap::new() };
                seen.save(fid);
                seen
            }
        }
    }

    fn save(&self, fid: u64) {
        if let (Some(storage), Ok(text)) = (window().local_storage().ok().flatten(), serde_json::to_string(self)) {
            let _ = storage.set_item(&Seen::storage_key(fid), &text);
        }
    }

    fn of(&self, id: &str) -> i64 {
        self.at.get(id).copied().unwrap_or(self.since)
    }

    fn saw(&mut self, id: &str, at: i64) {
        let seen = self.at.entry(id.to_owned()).or_insert(at);
        *seen = (*seen).max(at);
        if self.at.len() > SEEN_KEPT {
            let mut times: Vec<i64> = self.at.values().copied().collect();
            times.sort_unstable();
            let cut = times[times.len() - SEEN_KEPT];
            self.at.retain(|_, at| *at >= cut);
        }
    }
}

/// The reader's conversations and groups, as listed so far.
#[derive(Clone, Debug, Default, PartialEq)]
struct Listed {
    conversations: Vec<Conversation>,
    groups: Vec<Group>,
    /// Where the next page of conversations starts.
    more: Option<String>,
    loaded: bool,
    problem: Option<String>,
}

/// A row of the list, as it shows.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    thread: Thread,
    href: String,
    picture: Option<String>,
    name: String,
    preview: String,
    at: i64,
    unread: bool,
}

/// Reading: the conversations, beside the one open (or under it, on a
/// phone, one at a time).
#[component]
fn Conversations(fid: u64, key: StoredValue<String>, backend: Backend) -> impl IntoView {
    let session = use_session();
    let people = People::new(session.user.get_untracked());
    let listed = RwSignal::new(Listed::default());
    let seen = RwSignal::new(Seen::load(fid));
    let live = RwSignal::new(String::new());
    let conversation = use_query_param("c");
    let group = use_query_param("g");
    let writing = use_query_param("new");
    let open =
        Memo::new(move |_| conversation.get().map(Thread::Conversation).or_else(|| group.get().map(Thread::Group)));
    let paging = RwSignal::new(false);

    // Read the newest page, or the one after `cursor` ("Show more"); and
    // the groups, the first time.
    let load = move |cursor: Option<String>, groups: bool| {
        spawn_local(async move {
            let key = key.get_value();
            let call = Call::conversations(Category::Default, cursor.as_deref());
            let page = transport::call::<Page<ConversationList>>(backend, &key, &call, None).await;
            let groups = if groups {
                transport::call::<Page<GroupList>>(backend, &key, &Call::groups(Category::Default, None), None)
                    .await
                    .ok()
                    .map(|page| page.result.groups)
            } else {
                None
            };
            paging.set(false);
            match page {
                Ok(page) => {
                    let next = page.cursor().map(str::to_owned);
                    let fresh = page.result.conversations;
                    people.want(fresh.iter().flat_map(|c| c.participant_fids.clone()));
                    if let Some(groups) = &groups {
                        people.want(groups.iter().filter_map(|g| g.last_message.as_ref().map(|m| m.sender_fid)));
                    }
                    let before = listed.with_untracked(|l| l.loaded);
                    let open_now = open.get_untracked();
                    let mut arrived = Vec::new();
                    listed.update(|l| {
                        for conversation in fresh {
                            match l.conversations.iter_mut().find(|c| c.conversation_id == conversation.conversation_id)
                            {
                                Some(kept) => {
                                    if kept.changed_at() < conversation.changed_at() {
                                        arrived.push(conversation.clone());
                                    }
                                    *kept = conversation;
                                }
                                None => {
                                    if before && cursor.is_none() {
                                        arrived.push(conversation.clone());
                                    }
                                    l.conversations.push(conversation);
                                }
                            }
                        }
                        l.conversations.sort_by_key(|c| std::cmp::Reverse(c.changed_at()));
                        if let Some(groups) = groups {
                            l.groups = groups;
                        }
                        if cursor.is_some() || !l.loaded {
                            l.more = next;
                        }
                        l.loaded = true;
                        l.problem = None;
                    });
                    // Say what came, but not of the conversation open, which
                    // says so itself.
                    let news: Vec<String> = arrived
                        .iter()
                        .filter(|c| open_now.as_ref().map(Thread::id) != Some(c.conversation_id.as_str()))
                        .filter_map(|c| c.last_message.as_ref())
                        .filter(|m| m.sender_fid != fid && !m.is_deleted)
                        .map(|m| format!("New message from {}.", people.handle(m.sender_fid)))
                        .collect();
                    if !news.is_empty() {
                        live.set(news.join(" "));
                    }
                }
                Err(failure) => listed.update(|l| {
                    l.loaded = true;
                    l.problem = Some(failure.message(None));
                }),
            }
        });
    };
    load(None, true);
    poll_while_visible(move || load(None, false));
    let on_sent = Callback::new(move |()| load(None, false));

    let rows = move || {
        let seen = seen.get();
        listed.with(|l| {
            let mut rows: Vec<Row> = l
                .conversations
                .iter()
                .map(|c| {
                    let other = c.counterpart(fid).and_then(|fid| people.get(fid));
                    let name = match (&other, c.counterpart(fid)) {
                        (Some(user), _) => user.name().to_owned(),
                        (None, Some(fid)) => format!("fid {fid}"),
                        (None, None) => "Conversation".to_owned(),
                    };
                    Row {
                        thread: Thread::Conversation(c.conversation_id.clone()),
                        href: conversation_href(&c.conversation_id),
                        picture: other.and_then(|u| u.pfp_url),
                        name,
                        preview: preview(c.last_message.as_ref(), fid, None),
                        at: c.changed_at(),
                        unread: dc::unread(c, fid, seen.of(&c.conversation_id)),
                    }
                })
                .collect();
            rows.extend(l.groups.iter().map(|g| Row {
                thread: Thread::Group(g.group_id.clone()),
                href: group_href(&g.group_id),
                picture: g.image_url.clone(),
                name: if g.name.is_empty() { "Group".to_owned() } else { g.name.clone() },
                preview: preview(g.last_message.as_ref(), fid, Some(people)),
                at: g.changed_at(),
                unread: g.last_message.as_ref().is_some_and(|m| {
                    m.sender_fid != fid && !m.is_deleted && m.creation_timestamp > seen.of(&g.group_id)
                }),
            }));
            rows.sort_by_key(|r| std::cmp::Reverse(r.at));
            rows
        })
    };

    let list = move || {
        let (loaded, problem, empty, more) = listed
            .with(|l| (l.loaded, l.problem.clone(), l.conversations.is_empty() && l.groups.is_empty(), l.more.clone()));
        if !loaded {
            return view! {
                <ul class="dm-rows" aria-hidden="true">
                    {(0..4).map(|_| view! { <li class="dm-row-item"><span class="dm-row skeleton"></span></li> }).collect_view()}
                </ul>
            }
            .into_any();
        }
        if empty {
            return match problem {
                Some(why) => view! { <p class="dm-list-note" role="alert">{why}</p> }.into_any(),
                None => view! { <p class="dm-list-note muted">"No Direct Casts yet."</p> }.into_any(),
            };
        }
        view! {
            {problem.map(|why| view! { <p class="dm-list-note" role="alert">{why}</p> })}
            <ul class="dm-rows">
                <For
                    each=rows
                    key=|row| (row.thread.clone(), row.at, row.unread, row.name.clone(), row.preview.clone(), row.picture.clone())
                    children=move |row: Row| {
                        let thread = row.thread.clone();
                        let current = move || (open.get().as_ref() == Some(&thread)).then_some("page");
                        view! {
                            <li class="dm-row-item">
                                <a class="dm-row" class:dm-unread=row.unread href=row.href aria-current=current data-noscroll="">
                                    <Avatar url=row.picture size=44/>
                                    <span class="dm-row-text">
                                        <span class="dm-row-top">
                                            <span class="dm-row-name">{row.name}</span>
                                            <time class="dm-row-time" datetime=iso_utc(row.at)>{short_time(row.at)}</time>
                                        </span>
                                        <span class="dm-row-last">
                                            {row.unread.then(|| view! { <span class="visually-hidden">"Unread: "</span> })}
                                            {row.preview}
                                        </span>
                                    </span>
                                    {row.unread.then(|| view! { <span class="dm-dot" aria-hidden="true"></span> })}
                                </a>
                            </li>
                        }
                    }
                />
            </ul>
            {more.map(|cursor| {
                let cursor = StoredValue::new(cursor);
                view! {
                    <button
                        type="button"
                        class="button dm-more"
                        disabled=move || paging.get()
                        on:click=move |_| {
                            paging.set(true);
                            load(Some(cursor.get_value()), false);
                        }
                    >
                        "Show more"
                    </button>
                }
            })}
        }
        .into_any()
    };

    let pane = move || match (open.get(), writing.get().is_some()) {
        (Some(thread), _) => {
            view! { <ThreadView thread key backend people me=fid listed seen live on_sent/> }.into_any()
        }
        (None, true) => view! { <NewMessage mode=Mode::Read(backend) key people on_sent/> }.into_any(),
        (None, false) => view! {
            <div class="dm-empty">
                <p class="muted">"Pick a conversation, or write a new message."</p>
            </div>
        }
        .into_any(),
    };

    view! {
        <div class="dm-box" class:dm-has-pane=move || open.with(Option::is_some) || writing.with(Option::is_some)>
            <nav class="dm-list" aria-label="Conversations">
                <div class="dm-list-head">
                    <h2 class="dm-list-title">"Conversations"</h2>
                    <a class="button dm-new-button" href=new_message_href() data-noscroll="">"New message"</a>
                </div>
                {list}
            </nav>
            <div class="dm-pane">{pane}</div>
        </div>
        <KeyLine fid/>
        <p class="visually-hidden" role="status" aria-live="polite">{move || live.get()}</p>
    }
}

/// What a row shows of a last message.
fn preview(last: Option<&Message>, me: u64, people: Option<People>) -> String {
    match last {
        None => "No messages yet".to_owned(),
        Some(m) if m.is_deleted => "Message deleted".to_owned(),
        Some(m) => {
            let text = truncate_title(&m.message.replace('\n', " "), PREVIEW_CHARS);
            match people {
                _ if m.sender_fid == me => format!("You: {text}"),
                Some(people) => format!("{}: {text}", people.handle(m.sender_fid)),
                None => text,
            }
        }
    }
}

/// A message on its way, as the page shows it until the thread has it.
#[derive(Clone, Debug, PartialEq)]
struct Outgoing {
    /// The idempotency key it is sent with, every time: a retry sends
    /// nothing twice.
    id: String,
    text: String,
    at: i64,
    state: Going,
}

#[derive(Clone, Debug, PartialEq)]
enum Going {
    Sending,
    /// Sent: the message's id, when the API said it.
    Sent(Option<String>),
    Failed(String),
}

impl Outgoing {
    fn new(text: String) -> Outgoing {
        let id = window()
            .crypto()
            .map(|c| c.random_uuid())
            .unwrap_or_else(|_| format!("{}-{}", now(), js_sys::Math::random()));
        Outgoing { id, text, at: now(), state: Going::Sending }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Load {
    Loading,
    Ready,
    Failed(String),
}

/// A conversation's or a group's messages, oldest first, with older ones a
/// page at a time, and the composer.
#[component]
#[allow(clippy::too_many_arguments)]
fn ThreadView(
    thread: Thread,
    key: StoredValue<String>,
    backend: Backend,
    people: People,
    me: u64,
    listed: RwSignal<Listed>,
    seen: RwSignal<Seen>,
    live: RwSignal<String>,
    on_sent: Callback<()>,
) -> impl IntoView {
    let id = thread.id().to_owned();
    let is_group = matches!(thread, Thread::Group(_));
    let thread = StoredValue::new(thread);
    let messages = RwSignal::new(Vec::<Message>::new());
    let older = RwSignal::new(None::<String>);
    let state = RwSignal::new(Load::Loading);
    let paging = RwSignal::new(false);
    let outgoing = RwSignal::new(Vec::<Outgoing>::new());
    let scroller = NodeRef::<leptos::html::Div>::new();
    // A conversation not in the list (a link to it): its participants.
    let participants = RwSignal::new(None::<Vec<u64>>);
    let other = {
        let id = id.clone();
        Memo::new(move |_| {
            listed
                .with(|l| l.conversations.iter().find(|c| c.conversation_id == id).and_then(|c| c.counterpart(me)))
                .or_else(|| {
                    participants.get().and_then(|fids| {
                        fids.iter().copied().find(|fid| *fid != me).or_else(|| fids.contains(&me).then_some(me))
                    })
                })
        })
    };
    let group = {
        let id = id.clone();
        Memo::new(move |_| listed.with(|l| l.groups.iter().find(|g| g.group_id == id).cloned()))
    };
    if !is_group && listed.with_untracked(|l| !l.conversations.iter().any(|c| c.conversation_id == id)) {
        let id = id.clone();
        spawn_local(async move {
            let call = Call::conversation(&id);
            if let Ok(found) = transport::call::<Page<OneConversation>>(backend, &key.get_value(), &call, None).await {
                people.want(found.result.conversation.participant_fids.clone());
                participants.set(Some(found.result.conversation.participant_fids));
            }
        });
    }
    let title = Memo::new(move |_| match (is_group, other.get()) {
        (true, _) => group.get().map_or_else(|| "Group".to_owned(), |g| g.name),
        (false, Some(fid)) => people.get(fid).map_or_else(|| format!("fid {fid}"), |u| u.name().to_owned()),
        (false, None) => "Conversation".to_owned(),
    });
    let handle = Memo::new(move |_| other.get().map(|fid| people.handle(fid)));

    let thread_id = StoredValue::new(id.clone());
    let mark_seen = move || {
        if let Some(last) = messages.with_untracked(|m| m.last().map(|m| m.creation_timestamp)) {
            seen.update(|s| {
                thread_id.with_value(|id| s.saw(id, last));
                s.save(me);
            });
        }
    };
    let read_newest = move |first: bool| {
        spawn_local(async move {
            let call = Call::messages(&thread.get_value(), None);
            match transport::call::<Page<MessageList>>(backend, &key.get_value(), &call, None).await {
                Ok(page) => {
                    let had: HashSet<String> =
                        messages.with_untracked(|m| m.iter().map(|m| m.message_id.clone()).collect());
                    let at_end = near_end(scroller);
                    let next = page.cursor().map(str::to_owned);
                    let fresh = page.result.messages;
                    people.want(fresh.iter().map(|m| m.sender_fid));
                    let news: Vec<String> = fresh
                        .iter()
                        .filter(|m| !first && !had.contains(&m.message_id) && m.sender_fid != me && !m.is_deleted)
                        .map(|m| format!("New message from {}: {}", people.handle(m.sender_fid), m.message))
                        .collect();
                    messages.update(|m| dc::merge(m, fresh));
                    if first {
                        older.set(next);
                        state.set(Load::Ready);
                    }
                    // What the thread now has needn't show twice.
                    let ids: HashSet<String> =
                        messages.with_untracked(|m| m.iter().map(|m| m.message_id.clone()).collect());
                    outgoing.update(|o| o.retain(|g| !matches!(&g.state, Going::Sent(Some(id)) if ids.contains(id))));
                    if !news.is_empty() {
                        live.set(news.join(" "));
                    }
                    if first || at_end {
                        scroll_to_end(scroller);
                    }
                    mark_seen();
                }
                Err(failure) if first => state.set(Load::Failed(failure.message(None))),
                // A later read that fails: the thread stays as it was.
                Err(_) => {}
            }
        });
    };
    read_newest(true);
    poll_while_visible(move || read_newest(false));

    let read_older = move |_| {
        let Some(cursor) = older.get_untracked() else { return };
        paging.set(true);
        spawn_local(async move {
            let call = Call::messages(&thread.get_value(), Some(&cursor));
            match transport::call::<Page<MessageList>>(backend, &key.get_value(), &call, None).await {
                Ok(page) => {
                    people.want(page.result.messages.iter().map(|m| m.sender_fid));
                    older.set(page.cursor().map(str::to_owned));
                    messages.update(|m| dc::merge(m, page.result.messages));
                }
                Err(failure) => live.set(failure.message(None)),
            }
            paging.set(false);
        });
    };

    let deliver = move |going: Outgoing| {
        spawn_local(async move {
            let send = Send::to_thread(&thread.get_value(), &going.text);
            let result =
                transport::call::<Page<Sent>>(backend, &key.get_value(), &Call::send(&send), Some(&going.id)).await;
            let state = match result {
                Ok(sent) => Going::Sent(Some(sent.result.message_id)),
                Err(failure) => Going::Failed(failure.message(handle.get_untracked().as_deref())),
            };
            let sent = matches!(state, Going::Sent(_));
            outgoing.update(|o| {
                if let Some(g) = o.iter_mut().find(|g| g.id == going.id) {
                    g.state = state;
                }
            });
            if sent {
                read_newest(false);
                on_sent.run(());
            } else {
                // Why it didn't go, and Try again, in view.
                scroll_to_end(scroller);
            }
        });
    };
    let send = Callback::new(move |text: String| {
        let going = Outgoing::new(text);
        outgoing.update(|o| o.push(going.clone()));
        scroll_to_end(scroller);
        deliver(going);
    });
    let retry = Callback::new(move |id: String| {
        let Some(going) = outgoing.with_untracked(|o| o.iter().find(|g| g.id == id).cloned()) else { return };
        outgoing.update(|o| {
            if let Some(g) = o.iter_mut().find(|g| g.id == id) {
                g.state = Going::Sending;
            }
        });
        deliver(going);
    });
    let discard = Callback::new(move |id: String| outgoing.update(|o| o.retain(|g| g.id != id)));

    let profile = move || other.get().and_then(|fid| people.get(fid));
    let in_app = if is_group { INBOX_URL.to_owned() } else { dc::conversation_url(&id) };
    let label = move || format!("Messages with {}", title.get());
    view! {
        <section class="dm-thread" aria-label=label>
            <header class="dm-thread-head">
                <a class="dm-back" href=messages_href() data-noscroll="" aria-label="Back to your conversations">
                    <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d="M10 3 5 8l5 5"/></svg>
                </a>
                {move || {
                    let picture = if is_group { group.get().and_then(|g| g.image_url) } else { profile().and_then(|u| u.pfp_url) };
                    view! { <Avatar url=picture size=36/> }
                }}
                <div class="dm-thread-who">
                    <h2 class="dm-thread-title">{move || title.get()}</h2>
                    {move || profile().map(|user| view! {
                        <a class="dm-thread-handle muted" href=user_href(&user.username)>"@"{user.username.clone()}</a>
                    })}
                </div>
                <a class="dm-open-app" href=in_app target="_blank" rel="noopener">"Open in Farcaster"</a>
            </header>
            // Focusable, so the keyboard can scroll it.
            <div class="dm-scroll" node_ref=scroller tabindex="0">
                {move || match state.get() {
                    Load::Loading => view! { <p class="dm-empty muted" role="status">"Loading messages…"</p> }.into_any(),
                    Load::Failed(why) => view! { <p class="dm-empty" role="alert">{why}</p> }.into_any(),
                    Load::Ready => view! {
                        {move || older.with(Option::is_some).then(|| view! {
                            <button type="button" class="button dm-older" disabled=move || paging.get() on:click=read_older>
                                {move || if paging.get() { "Loading…" } else { "Load older messages" }}
                            </button>
                        })}
                        {move || (messages.with(Vec::is_empty) && outgoing.with(Vec::is_empty)).then(|| view! {
                            <p class="dm-empty muted">"No messages yet. Say hello."</p>
                        })}
                        <ol class="dm-messages" aria-label=label>
                            <For
                                each=move || messages.get()
                                key=|m| (m.message_id.clone(), m.is_deleted, m.message.clone())
                                children=move |m: Message| {
                                    let mine = m.sender_fid == me;
                                    let sender = (is_group && !mine).then(|| people.handle(m.sender_fid));
                                    view! {
                                        <li class="dm-message" class:dm-mine=mine>
                                            {sender.map(|who| view! { <p class="dm-sender muted">{who}</p> })}
                                            {if m.is_deleted {
                                                view! { <p class="dm-bubble dm-deleted">"Message deleted"</p> }.into_any()
                                            } else {
                                                view! { <p class="dm-bubble">{m.message.clone()}</p> }.into_any()
                                            }}
                                            <time class="dm-time" datetime=iso_utc(m.creation_timestamp)>{short_time(m.creation_timestamp)}</time>
                                        </li>
                                    }
                                }
                            />
                            <For
                                each=move || outgoing.get()
                                key=|g| (g.id.clone(), format!("{:?}", g.state))
                                children=move |g: Outgoing| view! { <OutgoingMessage going=g retry discard/> }
                            />
                        </ol>
                    }
                    .into_any(),
                }}
            </div>
            <Composer label=Signal::derive(move || format!("Message {}", handle.get().unwrap_or_else(|| title.get()))) on_send=send/>
        </section>
    }
}

/// A message on its way: Sending…, Sent, or why it didn't go, with Try
/// again and Discard.
#[component]
fn OutgoingMessage(going: Outgoing, retry: Callback<String>, discard: Callback<String>) -> impl IntoView {
    let failed = matches!(going.state, Going::Failed(_));
    let id = going.id.clone();
    let again = id.clone();
    view! {
        <li class="dm-message dm-mine dm-outgoing" class:dm-failed=failed>
            <p class="dm-bubble">{going.text.clone()}</p>
            {match going.state {
                Going::Sending => view! { <p class="dm-time">"Sending…"</p> }.into_any(),
                Going::Sent(_) => view! { <time class="dm-time" datetime=iso_utc(going.at)>{short_time(going.at)}" · Sent"</time> }.into_any(),
                Going::Failed(why) => view! {
                    <p class="dm-failure" role="alert">{why}</p>
                    <p class="dm-failure-actions">
                        <button type="button" class="link-button" on:click=move |_| retry.run(again.clone())>"Try again"</button>
                        <button type="button" class="link-button" on:click=move |_| discard.run(id.clone())>"Discard"</button>
                    </p>
                }
                .into_any(),
            }}
        </li>
    }
}

/// The text box: Enter sends, Shift+Enter starts a new line; up to 1,024
/// characters, counted down from 900.
#[component]
fn Composer(#[prop(into)] label: Signal<String>, on_send: Callback<String>) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let problem = RwSignal::new(None::<String>);
    let area = NodeRef::<leptos::html::Textarea>::new();
    let length = Memo::new(move |_| text.with(|t| dc::text_length(t.trim())));
    let fit = move || {
        if let Some(area) = area.get_untracked() {
            let style = web_sys::HtmlElement::style(&area);
            let _ = style.set_property("height", "auto");
            let _ = style.set_property("height", &format!("{}px", area.scroll_height().clamp(44, 168)));
        }
    };
    let submit = move || match dc::check_text(&text.get_untracked()) {
        Ok(checked) => {
            on_send.run(checked.to_owned());
            text.set(String::new());
            problem.set(None);
            request_animation_frame(fit);
        }
        Err(why) => problem.set(Some(why.message())),
    };
    let left = move || {
        let length = length.get();
        (length >= 900).then(|| {
            if length > MAX_CHARS {
                format!("{} over", length - MAX_CHARS)
            } else {
                format!("{} left", MAX_CHARS - length)
            }
        })
    };
    view! {
        <form
            class="dm-composer"
            on:submit=move |ev: ev::SubmitEvent| {
                ev.prevent_default();
                submit();
            }
        >
            <label class="visually-hidden" for="dm-text">{move || label.get()}</label>
            <textarea
                id="dm-text"
                class="dm-text"
                node_ref=area
                rows="1"
                placeholder="Write a message"
                aria-describedby="dm-left"
                prop:value=move || text.get()
                on:input=move |ev| {
                    text.set(event_target_value(&ev));
                    problem.set(None);
                    fit();
                }
                on:keydown=move |ev: ev::KeyboardEvent| {
                    if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
                        ev.prevent_default();
                        submit();
                    }
                }
            ></textarea>
            <span id="dm-left" class="dm-left" class:dm-over=move || { length.get() > MAX_CHARS }>{left}</span>
            <button type="submit" class="button primary dm-send" disabled=move || { let n = length.get(); n == 0 || n > MAX_CHARS }>
                "Send"
            </button>
            <p class="dm-problem" role="alert">{move || problem.get()}</p>
        </form>
    }
}

/// Whether a new message reads what it sends: reading through `Backend`, or
/// sending only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Read(Backend),
    SendOnly,
}

/// A message to someone found by name. Reading, it goes into their
/// conversation with the reader, which then opens; sending only, it goes
/// with Programmable DCs, and shows here as sent.
#[component]
fn NewMessage(
    mode: Mode,
    key: StoredValue<String>,
    people: People,
    #[prop(optional)] on_sent: Option<Callback<()>>,
) -> impl IntoView {
    let to = RwSignal::new(None::<UserSummary>);
    let outgoing = RwSignal::new(Vec::<Outgoing>::new());
    let go = StoredValue::new_local(use_go());
    let deliver = move |user: UserSummary, going: Outgoing| {
        spawn_local(async move {
            let handle = format!("@{}", user.username);
            let result = match mode {
                Mode::Read(backend) => {
                    let send = Send::to_account(user.fid, &going.text);
                    transport::call::<Page<Sent>>(backend, &key.get_value(), &Call::send(&send), Some(&going.id))
                        .await
                        .map(|sent| (Some(sent.result.message_id), sent.result.conversation_id))
                }
                Mode::SendOnly => {
                    transport::ext_send(&key.get_value(), user.fid, &going.text, &going.id).await.map(|()| (None, None))
                }
            };
            let state = match result {
                Ok((message, Some(conversation))) => {
                    if let Some(on_sent) = on_sent {
                        on_sent.run(());
                    }
                    go.with_value(|go| {
                        go(&conversation_href(&conversation), NavigateOptions { scroll: false, ..Default::default() })
                    });
                    Going::Sent(message)
                }
                Ok((message, None)) => Going::Sent(message),
                Err(failure) => Going::Failed(failure.message(Some(&handle))),
            };
            outgoing.update(|o| {
                if let Some(g) = o.iter_mut().find(|g| g.id == going.id) {
                    g.state = state;
                }
            });
        });
    };
    let send = Callback::new(move |text: String| {
        let Some(user) = to.get_untracked() else { return };
        let going = Outgoing::new(text);
        outgoing.update(|o| o.push(going.clone()));
        deliver(user, going);
    });
    let retry = Callback::new(move |id: String| {
        let (Some(user), Some(going)) =
            (to.get_untracked(), outgoing.with_untracked(|o| o.iter().find(|g| g.id == id).cloned()))
        else {
            return;
        };
        outgoing.update(|o| {
            if let Some(g) = o.iter_mut().find(|g| g.id == id) {
                g.state = Going::Sending;
            }
        });
        deliver(user, going);
    });
    let discard = Callback::new(move |id: String| outgoing.update(|o| o.retain(|g| g.id != id)));
    let picked = Callback::new(move |user: UserSummary| {
        outgoing.set(Vec::new());
        to.set(Some(user));
        request_animation_frame(|| {
            if let Some(area) = document().get_element_by_id("dm-text") {
                let _ = wasm_bindgen::JsCast::unchecked_into::<web_sys::HtmlElement>(area).focus();
            }
        });
    });
    view! {
        <section class="dm-thread dm-new" aria-labelledby="dm-new-title">
            <header class="dm-thread-head">
                {matches!(mode, Mode::Read(_)).then(|| view! {
                    <a class="dm-back" href=messages_href() data-noscroll="" aria-label="Back to your conversations">
                        <svg class="icon" viewBox="0 0 16 16" aria-hidden="true"><path d="M10 3 5 8l5 5"/></svg>
                    </a>
                })}
                <h2 id="dm-new-title" class="dm-thread-title">"New message"</h2>
            </header>
            <div class="dm-scroll">
                {move || match to.get() {
                    None => view! { <PersonPicker people picked/> }.into_any(),
                    Some(user) => view! {
                        <p class="dm-to">
                            <span class="muted">"To "</span>
                            <Avatar url=user.pfp_url.clone() size=28/>
                            <strong>{user.name().to_owned()}</strong>
                            <span class="muted">"@"{user.username.clone()}</span>
                            <button type="button" class="link-button" on:click=move |_| to.set(None)>"Change"</button>
                        </p>
                    }
                    .into_any(),
                }}
                <ol class="dm-messages" aria-label="Sent from here">
                    <For
                        each=move || outgoing.get()
                        key=|g| (g.id.clone(), format!("{:?}", g.state))
                        children=move |g: Outgoing| view! { <OutgoingMessage going=g retry discard/> }
                    />
                </ol>
                {(mode == Mode::SendOnly).then(|| view! {
                    <p class="dm-note muted">
                        "Sent with Farcaster's older send endpoint, from this browser. Replies come to your "
                        <a href=INBOX_URL target="_blank" rel="noopener">"inbox in Farcaster"</a>"."
                    </p>
                })}
            </div>
            {move || to.get().map(|user| view! {
                <Composer label=format!("Message @{}", user.username) on_send=send/>
            })}
        </section>
    }
}

/// Run `tick` every [`POLL`] while the page is in view, and at once when it
/// comes back into view; until the view that asked goes.
fn poll_while_visible(tick: impl Fn() + Clone + 'static) {
    let regular = tick.clone();
    let every = set_interval_with_handle(
        move || {
            if !document().hidden() {
                regular();
            }
        },
        POLL,
    )
    .ok();
    let shown = window_event_listener_untyped("visibilitychange", move |_| {
        if !document().hidden() {
            tick();
        }
    });
    on_cleanup(move || {
        if let Some(every) = every {
            every.clear();
        }
        shown.remove();
    });
}

/// Whether the thread is scrolled to (or near) its end.
fn near_end(scroller: NodeRef<leptos::html::Div>) -> bool {
    scroller.get_untracked().is_none_or(|el| el.scroll_height() - el.scroll_top() - el.client_height() < NEAR_END_PX)
}

/// Scroll the thread to its newest message, once it is drawn.
fn scroll_to_end(scroller: NodeRef<leptos::html::Div>) {
    request_animation_frame(move || {
        if let Some(el) = scroller.get_untracked() {
            el.set_scroll_top(el.scroll_height());
        }
    });
}

/// A time as a list or a message shows it, in the reader's time zone:
/// "09:41" today, "21 Oct" this year, "21 Oct 2025" before.
pub(super) fn short_time(at: i64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(at as f64 * 1000.0));
    let today = js_sys::Date::new_0();
    let options = js_sys::Object::new();
    let fields: &[(&str, &str)] = if date.to_date_string() == today.to_date_string() {
        &[("hour", "2-digit"), ("minute", "2-digit"), ("hourCycle", "h23")]
    } else if date.get_full_year() == today.get_full_year() {
        &[("day", "numeric"), ("month", "short")]
    } else {
        &[("day", "numeric"), ("month", "short"), ("year", "numeric")]
    };
    for (key, value) in fields {
        let _ = js_sys::Reflect::set(&options, &(*key).into(), &(*value).into());
    }
    date.to_locale_string("en-GB", &options).into()
}

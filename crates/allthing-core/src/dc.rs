// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct casts (DCs): the Farcaster app's private messages. They are not
//! part of the protocol: the app keeps them on its own servers, and another
//! client reaches them only through its API at `api.farcaster.xyz`, with the
//! reader's own API key (`wc_secret_…`, made at
//! farcaster.xyz/~/developers/api-keys). docs/DIRECT_CASTS.md says what
//! exists, what each part needs, and what a browser may call.
//!
//! - **The Direct Cast API** (a beta): `/fc/conversation-list`,
//!   `/fc/conversation`, `/fc/message-list`, `/fc/message` and
//!   `/fc/group-list` ([`Endpoint`], [`Call`]): a reader's conversations,
//!   their messages, and sending. It answers cross-origin requests only from
//!   the Farcaster app's own sites (and localhost), so a page on
//!   allthing.social can't read it itself.
//! - **Programmable DCs v1**: `PUT /v2/ext-send-direct-cast` ([`ExtSend`]),
//!   one message to one account, which answers any origin, so the reader's
//!   browser can send with it. Announced for deprecation since 2024.
//! - **Hand-off links** ([`INBOX_URL`], [`conversation_url`],
//!   [`create_url`]): the Farcaster app's inbox, a conversation in it, or its
//!   composer for a message to an account.
//!
//! The shapes here follow the API's beta reference (Notion, "Direct Cast
//! API Reference [Public]", last edited 4 April 2025), not captured traffic.

use serde::{Deserialize, Serialize};

use crate::api::API_PREFIX;
use crate::params::percent_encode;

/// The Farcaster app's API.
pub const API: &str = "https://api.farcaster.xyz";
/// Programmable DCs v1's send endpoint, the one a browser may call.
pub const EXT_SEND_PATH: &str = "/v2/ext-send-direct-cast";
/// The start of every key the Farcaster app makes.
pub const KEY_PREFIX: &str = "wc_secret_";
/// The longest key taken: well over what the app makes, short enough for a
/// header.
pub const MAX_KEY_LEN: usize = 200;
/// The longest message, in UTF-16 units, as the app's JavaScript counts
/// characters: an emoji counts as two.
pub const MAX_CHARS: usize = 1024;
/// The largest fid a message names: JSON numbers stay exact up to 2^53 − 1.
pub const MAX_FID: u64 = (1 << 53) - 1;
/// Where a reader makes, and revokes, an API key.
pub const API_KEYS_URL: &str = "https://farcaster.xyz/~/developers/api-keys";
/// Where a reader turns on the app's developer tools, if API Keys is hidden.
pub const DEVELOPER_TOOLS_URL: &str = "https://farcaster.xyz/~/settings/developer-tools";
/// The Farcaster app's inbox.
pub const INBOX_URL: &str = "https://farcaster.xyz/~/inbox";
/// Conversations asked for at a time.
pub const CONVERSATIONS_PER_PAGE: u32 = 25;
/// Messages asked for at a time.
pub const MESSAGES_PER_PAGE: u32 = 30;

/// Where the Worker relays the Direct Cast API, when its owner turns that
/// on: `/api/v1/dc/<endpoint>`. `GET /api/v1/dc` itself says whether it is
/// on (200) or not (404).
pub fn relay_path() -> String {
    format!("{API_PREFIX}/dc")
}

/// The header the browser hands the reader's key to the relay in, which
/// sends it on to the API as `Authorization: Bearer <key>`.
pub const KEY_HEADER: &str = "x-dc-key";
/// The header that marks the relay's answers that are the API's
/// (`upstream`): its own, and those saying it gave none in time (502, 503,
/// 504), as against the relay's refusals (turned off, no session, too many
/// calls), whose statuses would otherwise read as the API's.
pub const RELAY_HEADER: &str = "x-allthing-relay";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Put,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Put => "PUT",
        }
    }
}

/// The Direct Cast API's endpoints that Allthing uses: reading a reader's
/// conversations and groups, a conversation's messages, and sending one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    ConversationList,
    Conversation,
    MessageList,
    Message,
    GroupList,
}

impl Endpoint {
    pub const ALL: [Endpoint; 5] = [
        Endpoint::ConversationList,
        Endpoint::Conversation,
        Endpoint::MessageList,
        Endpoint::Message,
        Endpoint::GroupList,
    ];

    /// Its name, as the last part of its path: `conversation-list`.
    pub fn name(self) -> &'static str {
        match self {
            Endpoint::ConversationList => "conversation-list",
            Endpoint::Conversation => "conversation",
            Endpoint::MessageList => "message-list",
            Endpoint::Message => "message",
            Endpoint::GroupList => "group-list",
        }
    }

    /// Its path on the API: `/fc/conversation-list`.
    pub fn path(self) -> String {
        format!("/fc/{}", self.name())
    }

    /// The endpoint called `name`.
    pub fn named(name: &str) -> Option<Endpoint> {
        Endpoint::ALL.into_iter().find(|endpoint| endpoint.name() == name)
    }

    /// The one method Allthing calls it with: GET reads, PUT sends.
    pub fn method(self) -> Method {
        match self {
            Endpoint::Message => Method::Put,
            _ => Method::Get,
        }
    }

    /// The query parameters it takes from Allthing.
    pub fn query_keys(self) -> &'static [&'static str] {
        match self {
            Endpoint::ConversationList | Endpoint::GroupList => &["category", "cursor", "limit"],
            Endpoint::Conversation => &["conversationId"],
            Endpoint::MessageList => &["conversationId", "groupId", "cursor", "limit"],
            Endpoint::Message => &[],
        }
    }
}

/// Which of the app's inboxes a list is: the reader's own, message requests
/// from people they don't follow, or the archived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Category {
    #[default]
    Default,
    Request,
    Archived,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Default => "default",
            Category::Request => "request",
            Category::Archived => "archived",
        }
    }
}

/// Where messages are: a conversation between two accounts, or a group.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Thread {
    Conversation(String),
    Group(String),
}

impl Thread {
    pub fn id(&self) -> &str {
        match self {
            Thread::Conversation(id) | Thread::Group(id) => id,
        }
    }
}

/// One request to the Direct Cast API: what it calls, and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub endpoint: Endpoint,
    pub query: Vec<(&'static str, String)>,
    /// The JSON it sends (PUT).
    pub body: Option<String>,
}

impl Call {
    fn get(endpoint: Endpoint, query: Vec<(&'static str, Option<String>)>) -> Call {
        let query =
            query.into_iter().filter_map(|(key, value)| Some((key, value.filter(|v| !v.is_empty())?))).collect();
        Call { endpoint, query, body: None }
    }

    /// One conversation of the reader's: the cheapest read, which says
    /// whether the API answers at all, and accepts the key.
    pub fn probe() -> Call {
        Call::get(Endpoint::ConversationList, vec![("limit", Some("1".into()))])
    }

    /// A page of the reader's conversations, newest first.
    pub fn conversations(category: Category, cursor: Option<&str>) -> Call {
        Call::get(
            Endpoint::ConversationList,
            vec![
                ("category", Some(category.as_str().into())),
                ("cursor", cursor.map(str::to_owned)),
                ("limit", Some(CONVERSATIONS_PER_PAGE.to_string())),
            ],
        )
    }

    /// A page of the groups the reader administers, newest first: the API
    /// shows a group only to its admins.
    pub fn groups(category: Category, cursor: Option<&str>) -> Call {
        Call { endpoint: Endpoint::GroupList, ..Call::conversations(category, cursor) }
    }

    /// One conversation, with its participants.
    pub fn conversation(id: &str) -> Call {
        Call::get(Endpoint::Conversation, vec![("conversationId", Some(id.to_owned()))])
    }

    /// A page of a conversation's or group's messages, newest first.
    pub fn messages(thread: &Thread, cursor: Option<&str>) -> Call {
        let key = match thread {
            Thread::Conversation(_) => "conversationId",
            Thread::Group(_) => "groupId",
        };
        Call::get(
            Endpoint::MessageList,
            vec![
                (key, Some(thread.id().to_owned())),
                ("cursor", cursor.map(str::to_owned)),
                ("limit", Some(MESSAGES_PER_PAGE.to_string())),
            ],
        )
    }

    /// Send a message.
    pub fn send(send: &Send) -> Call {
        Call { endpoint: Endpoint::Message, query: Vec::new(), body: serde_json::to_string(send).ok() }
    }

    pub fn method(&self) -> Method {
        self.endpoint.method()
    }

    fn query_string(&self) -> String {
        let mut out = String::new();
        for (i, (key, value)) in self.query.iter().enumerate() {
            out.push(if i == 0 { '?' } else { '&' });
            out.push_str(key);
            out.push('=');
            out.push_str(&percent_encode(value));
        }
        out
    }

    /// Its address on the API at `origin` (the app's, [`API`]).
    pub fn url(&self, origin: &str) -> String {
        format!("{origin}{}{}", self.endpoint.path(), self.query_string())
    }

    /// Its address on the Worker's relay: `/api/v1/dc/conversation-list?…`.
    pub fn relay_url(&self) -> String {
        format!("{}/{}{}", relay_path(), self.endpoint.name(), self.query_string())
    }
}

/// A message, as the API lists it. A deleted one is kept, emptied.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Its conversation's id, when it isn't a group's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    pub message_id: String,
    pub sender_fid: u64,
    #[serde(default)]
    pub message: String,
    /// Unix seconds.
    pub creation_timestamp: i64,
    #[serde(default)]
    pub is_deleted: bool,
    /// Sent through the API, not typed in the app.
    #[serde(default)]
    pub is_programmatic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<Mention>,
}

/// An account a message names, by where in its text the name is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mention {
    pub fid: u64,
    pub text_index: usize,
    /// The name's length, its "@" included.
    pub length: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Days before a message is deleted: 1, 7, 30 (the default) or 365.
    #[serde(default, rename = "messageTTLDays", skip_serializing_if = "Option::is_none")]
    pub message_ttl_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members_can_invite: Option<bool>,
}

/// A conversation between two accounts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub conversation_id: String,
    #[serde(default)]
    pub participant_fids: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Settings>,
    #[serde(default)]
    pub creation_timestamp: i64,
    /// Its last message's time, or its creation's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_timestamp: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<Message>,
}

impl Conversation {
    /// The other account in it: `me` in a conversation with themselves.
    pub fn counterpart(&self, me: u64) -> Option<u64> {
        self.participant_fids
            .iter()
            .copied()
            .find(|&fid| fid != me)
            .or_else(|| self.participant_fids.contains(&me).then_some(me))
    }

    /// When it last changed: its last message, or its creation.
    pub fn changed_at(&self) -> i64 {
        self.last_message
            .as_ref()
            .map(|m| m.creation_timestamp)
            .or(self.last_modified_timestamp)
            .unwrap_or(self.creation_timestamp)
    }
}

/// A group chat (up to 1,000 members), as its admins see it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub group_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_link_url: Option<String>,
    #[serde(default)]
    pub admin_fids: Vec<u64>,
    #[serde(default)]
    pub member_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Settings>,
    #[serde(default)]
    pub creation_timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified_timestamp: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<Message>,
}

impl Group {
    /// When it last changed: its last message, or its creation.
    pub fn changed_at(&self) -> i64 {
        self.last_message
            .as_ref()
            .map(|m| m.creation_timestamp)
            .or(self.last_modified_timestamp)
            .unwrap_or(self.creation_timestamp)
    }
}

/// An answer: `{ "result": … }`, with `next.cursor` on a list that goes on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub result: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Next>,
}

impl<T> Page<T> {
    /// Where the next page starts, if there is one.
    pub fn cursor(&self) -> Option<&str> {
        self.next.as_ref().and_then(|n| n.cursor.as_deref()).filter(|c| !c.is_empty())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Next {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationList {
    #[serde(default)]
    pub conversations: Vec<Conversation>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupList {
    #[serde(default)]
    pub groups: Vec<Group>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageList {
    #[serde(default)]
    pub messages: Vec<Message>,
}

/// `GET /fc/conversation`'s result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OneConversation {
    pub conversation: Conversation,
}

/// What `PUT /fc/message` takes: a message for a conversation, a group or
/// an account (whose conversation with the reader it starts or goes on).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Send {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_fid: Option<u64>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to_message_id: Option<String>,
}

impl Send {
    /// `text` into `thread`.
    pub fn to_thread(thread: &Thread, text: &str) -> Send {
        let (conversation_id, group_id) = match thread {
            Thread::Conversation(id) => (Some(id.clone()), None),
            Thread::Group(id) => (None, Some(id.clone())),
        };
        Send { conversation_id, group_id, message: text.to_owned(), ..Send::default() }
    }

    /// `text` to the account `fid`.
    pub fn to_account(fid: u64, text: &str) -> Send {
        Send { recipient_fid: Some(fid), message: text.to_owned(), ..Send::default() }
    }
}

/// `PUT /fc/message`'s result: the new message's id, and its conversation's
/// when it was sent to an account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

/// What `PUT /v2/ext-send-direct-cast` takes. The idempotency key makes a
/// retry safe: the same key, recipient and text send nothing twice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtSend {
    pub recipient_fid: u64,
    pub message: String,
    pub idempotency_key: String,
}

/// Its result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtSent {
    #[serde(default)]
    pub success: bool,
}

/// The API's error: `{"errors":[{"message":"…"}]}`.
#[derive(Clone, Debug, Default, Deserialize)]
struct Errors {
    #[serde(default)]
    errors: Vec<ErrorMessage>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ErrorMessage {
    #[serde(default)]
    message: String,
}

/// What the API's error body says, if it says anything.
pub fn error_message(body: &str) -> Option<String> {
    let errors: Errors = serde_json::from_str(body).ok()?;
    errors.errors.into_iter().map(|e| e.message.trim().to_owned()).find(|m| !m.is_empty())
}

/// Why the API didn't do what was asked, from its status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// 401: the key is wrong, expired or revoked.
    Key,
    /// 403: on a send, the recipient's Direct Cast settings don't take the
    /// reader's messages (they may take only people they follow).
    NotAllowed,
    /// 404: no such conversation, or not the reader's.
    NotFound,
    /// 429: sending, or asking, too fast.
    TooFast,
    /// 5xx, or no answer: try again.
    Down,
    /// Anything else.
    Other(u16),
}

impl Refusal {
    /// The refusal a status is, if it is one.
    pub fn from_status(status: u16) -> Option<Refusal> {
        match status {
            200..=299 => None,
            401 => Some(Refusal::Key),
            403 => Some(Refusal::NotAllowed),
            404 => Some(Refusal::NotFound),
            429 => Some(Refusal::TooFast),
            500..=599 => Some(Refusal::Down),
            other => Some(Refusal::Other(other)),
        }
    }

    /// What to tell the reader, about a message to `to` ("@alice") when it
    /// was one.
    pub fn message(self, to: Option<&str>) -> String {
        match self {
            Refusal::Key => {
                "Farcaster didn't accept your API key: it may have expired or been revoked. Make a new one and paste it here."
                    .into()
            }
            Refusal::NotAllowed => match to {
                Some(to) => format!(
                    "{to}'s Direct Cast settings don't take messages from you: they may take messages only from people they follow."
                ),
                None => "Farcaster didn't allow that: the recipient's Direct Cast settings may not take messages from you.".into(),
            },
            Refusal::NotFound => "Farcaster has no such conversation for you.".into(),
            Refusal::TooFast => "Farcaster says that's too many messages for now. Wait a minute and try again.".into(),
            Refusal::Down => "Farcaster's API isn't answering just now. Try again shortly.".into(),
            Refusal::Other(status) => format!("Farcaster's API refused that (HTTP {status})."),
        }
    }
}

/// What's wrong with a pasted key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyProblem {
    Empty,
    /// It doesn't start with `wc_secret_`: not a key the app makes.
    NotAKey,
    TooLong,
    /// Spaces, line breaks or other characters a key doesn't have.
    Characters,
}

impl KeyProblem {
    pub fn message(self) -> &'static str {
        match self {
            KeyProblem::Empty => "Paste your API key.",
            KeyProblem::NotAKey => "That isn't a Farcaster API key: a key starts with wc_secret_.",
            KeyProblem::TooLong => "That's longer than a Farcaster API key.",
            KeyProblem::Characters => "A Farcaster API key has only letters, digits, _ and -.",
        }
    }
}

/// A pasted key, trimmed, if it looks like one the Farcaster app makes:
/// `wc_secret_` and then letters, digits, `_` or `-`. Nothing else may go
/// into a header.
pub fn check_key(pasted: &str) -> Result<String, KeyProblem> {
    let key = pasted.trim();
    if key.is_empty() {
        return Err(KeyProblem::Empty);
    }
    let Some(rest) = key.strip_prefix(KEY_PREFIX) else { return Err(KeyProblem::NotAKey) };
    if key.len() > MAX_KEY_LEN {
        return Err(KeyProblem::TooLong);
    }
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(KeyProblem::Characters);
    }
    Ok(key.to_owned())
}

/// A message's length as the API counts it: UTF-16 units.
pub fn text_length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// What's wrong with a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextProblem {
    Empty,
    TooLong { length: usize },
}

impl TextProblem {
    pub fn message(self) -> String {
        match self {
            TextProblem::Empty => "Write a message first.".into(),
            TextProblem::TooLong { length } => {
                format!("That's {length} characters: a Direct Cast takes {MAX_CHARS} at most.")
            }
        }
    }
}

/// A message as it's sent, trimmed, if it fits: not empty, and at most
/// [`MAX_CHARS`] long.
pub fn check_text(text: &str) -> Result<&str, TextProblem> {
    let text = text.trim();
    let length = text_length(text);
    if length == 0 {
        Err(TextProblem::Empty)
    } else if length > MAX_CHARS {
        Err(TextProblem::TooLong { length })
    } else {
        Ok(text)
    }
}

/// A recipient's fid, if it can be one: 1 up to [`MAX_FID`].
pub fn check_recipient(fid: u64) -> Option<u64> {
    (1..=MAX_FID).contains(&fid).then_some(fid)
}

/// A conversation in the Farcaster app's inbox.
pub fn conversation_url(conversation_id: &str) -> String {
    format!("{INBOX_URL}/{}", percent_encode(conversation_id))
}

/// The Farcaster app's composer for a message to `fid`, with `text` in it
/// when there is any: what Allthing hands a message to where it can't send.
pub fn create_url(fid: u64, text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        format!("{INBOX_URL}/create/{fid}")
    } else {
        format!("{INBOX_URL}/create/{fid}?text={}", percent_encode(text))
    }
}

/// Whether a conversation has a message the reader hasn't seen: its last
/// one is someone else's, not deleted, and newer than `seen` (Unix seconds,
/// when the reader last looked at it here).
pub fn unread(conversation: &Conversation, me: u64, seen: i64) -> bool {
    conversation
        .last_message
        .as_ref()
        .is_some_and(|m| m.sender_fid != me && !m.is_deleted && m.creation_timestamp > seen)
}

/// Add a page of messages (newest first, as the API lists them) to `thread`
/// (oldest first, as a thread shows them): each message once, the newer
/// copy kept, in time order.
pub fn merge(thread: &mut Vec<Message>, page: impl IntoIterator<Item = Message>) {
    for message in page {
        match thread.iter_mut().find(|m| m.message_id == message.message_id) {
            Some(kept) => *kept = message,
            None => thread.push(message),
        }
    }
    thread
        .sort_by(|a, b| a.creation_timestamp.cmp(&b.creation_timestamp).then_with(|| a.message_id.cmp(&b.message_id)));
}

// The Worker's relay (`allthing-worker`'s `dc_relay`), which its owner may
// turn on: what it passes on, checked here.

/// The largest body the relay takes. A message of [`MAX_CHARS`] is at most
/// about 6 KB of JSON (six bytes a UTF-16 unit when escaped), with room for
/// its ids.
pub const MAX_BODY: usize = 8 * 1024;
/// The longest id, cursor or idempotency key the relay passes on.
pub const MAX_ID_LEN: usize = 256;

/// Whether `value` can be an id or a cursor the relay passes on: not empty,
/// not too long, and no control characters.
fn is_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_ID_LEN && !value.chars().any(char::is_control)
}

/// A relayed read's query: of the pairs `asked` for, only the keys
/// `endpoint` takes, in the order it takes them, the first value of each,
/// and none empty or too long.
pub fn relay_query(endpoint: Endpoint, asked: &[(String, String)]) -> Vec<(&'static str, String)> {
    endpoint
        .query_keys()
        .iter()
        .filter_map(|&key| {
            let value = asked.iter().find(|(k, _)| k == key).map(|(_, v)| v)?;
            is_id(value).then(|| (key, value.clone()))
        })
        .collect()
}

/// An `idempotency-key` the relay passes on: 1 to 64 letters, digits, `-`
/// or `_` (the browser sends a UUID).
pub fn check_idempotency(value: &str) -> bool {
    (1..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Why the relay won't pass a message on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendProblem {
    /// Not JSON in the shape `PUT /fc/message` takes.
    Shape,
    /// No conversation, group or account to send it to, or more than one.
    Target,
    /// An id that can't be one: empty, too long, or a fid out of range.
    Id,
    Text(TextProblem),
}

impl SendProblem {
    pub fn message(self) -> String {
        match self {
            SendProblem::Shape => {
                "Expected a message: {\"conversationId\" or \"groupId\" or \"recipientFid\", \"message\"}".into()
            }
            SendProblem::Target => "A message goes to one conversation, group or account.".into(),
            SendProblem::Id => "That conversation, group, message or account id can't be one.".into(),
            SendProblem::Text(problem) => problem.message(),
        }
    }
}

/// A message for `PUT /fc/message`, as the relay passes it on: read from
/// `body`, checked, and written out again, so only [`Send`]'s fields go on,
/// the message trimmed as the browser sends it.
pub fn check_send(body: &str) -> Result<String, SendProblem> {
    let send: Send = serde_json::from_str(body).map_err(|_| SendProblem::Shape)?;
    let targets = [send.conversation_id.is_some(), send.group_id.is_some(), send.recipient_fid.is_some()];
    if targets.into_iter().filter(|&named| named).count() != 1 {
        return Err(SendProblem::Target);
    }
    let ids = [&send.conversation_id, &send.group_id, &send.in_reply_to_message_id];
    if ids.into_iter().flatten().any(|id| !is_id(id))
        || send.recipient_fid.is_some_and(|f| check_recipient(f).is_none())
    {
        return Err(SendProblem::Id);
    }
    let message = check_text(&send.message).map_err(SendProblem::Text)?.to_owned();
    serde_json::to_string(&Send { message, ..send }).map_err(|_| SendProblem::Shape)
}

#[cfg(test)]
mod tests {
    //! The fixtures (`tests/fixtures/dc-docs-*.json`) are shaped like the
    //! examples in the Direct Cast API's beta reference, filled in with
    //! made-up accounts: they are not captured responses, which Allthing has
    //! none of (its key would be a reader's). Each says so in a `_fixture`
    //! field, which the types ignore as they ignore any field they don't know.

    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn a_conversation_list_reads_as_the_docs_show_it() {
        let page: Page<ConversationList> = serde_json::from_str(&fixture("dc-docs-conversation-list.json")).unwrap();
        let conversations = &page.result.conversations;
        assert_eq!(conversations.len(), 2);
        let first = &conversations[0];
        assert_eq!(first.conversation_id, "636f6e7669643a31323639352d32");
        assert_eq!(first.participant_fids, vec![1234, 4321]);
        assert_eq!(first.settings.as_ref().and_then(|s| s.message_ttl_days), Some(7));
        let last = first.last_message.as_ref().unwrap();
        assert_eq!((last.sender_fid, last.message.as_str(), last.is_programmatic), (4321, "Hello, World", true));
        assert_eq!(last.conversation_id, None);
        assert_eq!(first.counterpart(1234), Some(4321));
        assert_eq!(first.changed_at(), 1_709_753_166);
        // The second has no message yet: it changed when it was made.
        assert_eq!(conversations[1].last_message, None);
        assert_eq!(conversations[1].changed_at(), 1_709_000_000);
        assert_eq!(page.cursor(), Some("eyJwYWdlIjoxLCJsaW1pdCI6MjV9"));
    }

    #[test]
    fn a_message_list_reads_with_deleted_messages_and_mentions() {
        let page: Page<MessageList> = serde_json::from_str(&fixture("dc-docs-message-list.json")).unwrap();
        let messages = &page.result.messages;
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].mentions, vec![Mention { fid: 3, text_index: 7, length: 8 }]);
        assert_eq!(messages[0].in_reply_to_message_id.as_deref(), Some("0da6e9861234827703a40246bb3aa8bc"));
        // Deleted: kept, emptied.
        assert!(messages[1].is_deleted && messages[1].message.is_empty());
        assert_eq!(page.cursor(), None);
    }

    #[test]
    fn a_group_list_and_a_sent_message_read_too() {
        let page: Page<GroupList> = serde_json::from_str(&fixture("dc-docs-group-list.json")).unwrap();
        let group = &page.result.groups[0];
        assert_eq!(
            (group.group_id.as_str(), group.name.as_str(), group.member_count),
            ("9f8201eebb1c888c", "Group Name", 3)
        );
        assert_eq!(group.admin_fids, vec![1, 4321]);
        assert_eq!(group.settings.as_ref().and_then(|s| s.members_can_invite), Some(false));
        assert_eq!(group.changed_at(), 1_709_753_166);
        let sent: Page<Sent> = serde_json::from_str(&fixture("dc-docs-sent.json")).unwrap();
        assert_eq!(sent.result.message_id, "0dd6e7060465817903a40148fb3aa7aa");
        assert_eq!(sent.result.conversation_id.as_deref(), Some("636f6e7669643a31323639352d32"));
        let ext: Page<ExtSent> = serde_json::from_str(r#"{"result":{"success":true}}"#).unwrap();
        assert!(ext.result.success);
    }

    #[test]
    fn calls_have_their_paths_queries_and_methods() {
        assert_eq!(Call::probe().url(API), "https://api.farcaster.xyz/fc/conversation-list?limit=1");
        assert_eq!(Call::probe().relay_url(), "/api/v1/dc/conversation-list?limit=1");
        assert_eq!(
            Call::conversations(Category::Request, Some("a b")).url(API),
            "https://api.farcaster.xyz/fc/conversation-list?category=request&cursor=a%20b&limit=25"
        );
        assert_eq!(
            Call::groups(Category::Default, None).relay_url(),
            "/api/v1/dc/group-list?category=default&limit=25"
        );
        let thread = Thread::Conversation("636f".into());
        assert_eq!(
            Call::messages(&thread, None).url(API),
            "https://api.farcaster.xyz/fc/message-list?conversationId=636f&limit=30"
        );
        assert_eq!(
            Call::messages(&Thread::Group("9f82".into()), Some("c")).relay_url(),
            "/api/v1/dc/message-list?groupId=9f82&cursor=c&limit=30"
        );
        assert_eq!(
            Call::conversation("636f").url("http://127.0.0.1:9"),
            "http://127.0.0.1:9/fc/conversation?conversationId=636f"
        );
        let send = Call::send(&Send::to_thread(&thread, "hi"));
        assert_eq!((send.method(), send.url(API).as_str()), (Method::Put, "https://api.farcaster.xyz/fc/message"));
        assert_eq!(send.body.as_deref(), Some(r#"{"conversationId":"636f","message":"hi"}"#));
        for endpoint in Endpoint::ALL {
            assert_eq!(Endpoint::named(endpoint.name()), Some(endpoint));
            assert_eq!(endpoint.path(), format!("/fc/{}", endpoint.name()));
            // Each call only uses the query keys its endpoint takes.
            for call in
                [Call::probe(), Call::conversations(Category::Archived, Some("x")), Call::messages(&thread, Some("y"))]
            {
                if call.endpoint == endpoint {
                    assert!(call.query.iter().all(|(key, _)| endpoint.query_keys().contains(key)), "{endpoint:?}");
                }
            }
        }
        assert_eq!(Endpoint::named("message-list"), Some(Endpoint::MessageList));
        assert_eq!(Endpoint::named("group"), None);
        assert_eq!(Endpoint::Message.method(), Method::Put);
        assert_eq!(Endpoint::ConversationList.method(), Method::Get);
    }

    #[test]
    fn sends_name_their_thread_or_account() {
        let to_group = serde_json::to_string(&Send::to_thread(&Thread::Group("9f82".into()), "gm")).unwrap();
        assert_eq!(to_group, r#"{"groupId":"9f82","message":"gm"}"#);
        let to_account = serde_json::to_string(&Send::to_account(6841, "gm")).unwrap();
        assert_eq!(to_account, r#"{"recipientFid":6841,"message":"gm"}"#);
        let ext = ExtSend { recipient_fid: 6841, message: "gm".into(), idempotency_key: "k".into() };
        assert_eq!(
            serde_json::to_string(&ext).unwrap(),
            r#"{"recipientFid":6841,"message":"gm","idempotencyKey":"k"}"#
        );
    }

    #[test]
    fn keys_are_the_apps_and_nothing_a_header_could_be_bent_by() {
        assert_eq!(check_key("  wc_secret_abc123_DEF-9 \n"), Ok("wc_secret_abc123_DEF-9".into()));
        assert_eq!(check_key(""), Err(KeyProblem::Empty));
        assert_eq!(check_key("   "), Err(KeyProblem::Empty));
        assert_eq!(check_key("sk_live_abc"), Err(KeyProblem::NotAKey));
        assert_eq!(check_key("Bearer wc_secret_abc"), Err(KeyProblem::NotAKey));
        assert_eq!(check_key("wc_secret_"), Err(KeyProblem::Characters));
        for bad in ["wc_secret_ab cd", "wc_secret_ab\r\nx-evil: 1", "wc_secret_ab:cd", "wc_secret_é"] {
            assert_eq!(check_key(bad), Err(KeyProblem::Characters), "{bad:?}");
        }
        assert_eq!(check_key(&format!("wc_secret_{}", "a".repeat(MAX_KEY_LEN))), Err(KeyProblem::TooLong));
        assert!(KeyProblem::NotAKey.message().contains("wc_secret_"));
    }

    #[test]
    fn messages_fit_in_1024_characters_as_the_app_counts_them() {
        assert_eq!(check_text("  gm \n"), Ok("gm"));
        assert_eq!(check_text(" \n\t"), Err(TextProblem::Empty));
        assert_eq!(check_text(&"a".repeat(MAX_CHARS)), Ok("a".repeat(MAX_CHARS).as_str()));
        assert_eq!(check_text(&"a".repeat(MAX_CHARS + 1)), Err(TextProblem::TooLong { length: 1025 }));
        // An emoji is two UTF-16 units, as JavaScript counts it.
        assert_eq!(text_length("🐸"), 2);
        assert_eq!(check_text(&"🐸".repeat(513)), Err(TextProblem::TooLong { length: 1026 }));
        assert_eq!(text_length("é"), 1);
        assert!(TextProblem::TooLong { length: 1100 }.message().contains("1024"));
    }

    #[test]
    fn recipients_are_real_fids() {
        assert_eq!(check_recipient(3), Some(3));
        assert_eq!(check_recipient(MAX_FID), Some(MAX_FID));
        assert_eq!(check_recipient(0), None);
        assert_eq!(check_recipient(MAX_FID + 1), None);
    }

    #[test]
    fn hand_off_links_open_the_apps_inbox_and_composer() {
        assert_eq!(INBOX_URL, "https://farcaster.xyz/~/inbox");
        assert_eq!(create_url(1, "gm"), "https://farcaster.xyz/~/inbox/create/1?text=gm");
        assert_eq!(
            create_url(6841, " hi there & bye "),
            "https://farcaster.xyz/~/inbox/create/6841?text=hi%20there%20%26%20bye"
        );
        assert_eq!(create_url(6841, "  "), "https://farcaster.xyz/~/inbox/create/6841");
        assert_eq!(conversation_url("636f6e76"), "https://farcaster.xyz/~/inbox/636f6e76");
        assert_eq!(conversation_url("a/b"), "https://farcaster.xyz/~/inbox/a%2Fb");
    }

    #[test]
    fn errors_and_statuses_say_why() {
        assert_eq!(
            error_message(r#"{"errors":[{"message":"Authentication required"}]}"#).as_deref(),
            Some("Authentication required")
        );
        assert_eq!(error_message(r#"{"errors":[]}"#), None);
        assert_eq!(error_message("<html>"), None);
        assert_eq!(Refusal::from_status(200), None);
        assert_eq!(Refusal::from_status(401), Some(Refusal::Key));
        assert_eq!(Refusal::from_status(403), Some(Refusal::NotAllowed));
        assert_eq!(Refusal::from_status(429), Some(Refusal::TooFast));
        assert_eq!(Refusal::from_status(502), Some(Refusal::Down));
        assert_eq!(Refusal::from_status(400), Some(Refusal::Other(400)));
        assert!(Refusal::NotAllowed.message(Some("@alice")).starts_with("@alice's Direct Cast settings"));
    }

    fn message(id: &str, sender: u64, at: i64) -> Message {
        Message {
            message_id: id.into(),
            sender_fid: sender,
            message: id.into(),
            creation_timestamp: at,
            ..Message::default()
        }
    }

    #[test]
    fn unread_is_someone_elses_newer_message() {
        let mut conversation =
            Conversation { conversation_id: "c".into(), participant_fids: vec![1, 2], ..Default::default() };
        assert!(!unread(&conversation, 1, 0));
        conversation.last_message = Some(message("m", 2, 100));
        assert!(unread(&conversation, 1, 99));
        assert!(!unread(&conversation, 1, 100));
        // The reader's own last message isn't unread, nor a deleted one.
        assert!(!unread(&conversation, 2, 0));
        conversation.last_message.as_mut().unwrap().is_deleted = true;
        assert!(!unread(&conversation, 1, 0));
        // With themselves: the reader is the counterpart.
        let alone = Conversation { participant_fids: vec![1], ..Default::default() };
        assert_eq!(alone.counterpart(1), Some(1));
        assert_eq!(Conversation::default().counterpart(1), None);
    }

    #[test]
    fn pages_merge_into_a_thread_once_each_in_time_order() {
        let mut thread = vec![];
        // The newest page first, as the API lists it, then an older one.
        merge(&mut thread, vec![message("c", 1, 30), message("b", 2, 20)]);
        merge(&mut thread, vec![message("b", 2, 20), message("a", 1, 10)]);
        let ids: Vec<_> = thread.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        // A newer copy of one (deleted since) takes its place.
        let mut deleted = message("b", 2, 20);
        deleted.is_deleted = true;
        deleted.message.clear();
        merge(&mut thread, vec![message("d", 2, 40), deleted]);
        assert_eq!(thread.len(), 4);
        assert!(thread[1].is_deleted);
        assert_eq!(thread[3].message_id, "d");
    }

    fn pairs(query: &[(&str, &str)]) -> Vec<(String, String)> {
        query.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn the_relay_passes_on_only_the_query_an_endpoint_takes() {
        let asked = pairs(&[("limit", "1"), ("evil", "1"), ("category", "default"), ("limit", "99"), ("cursor", "")]);
        let query = relay_query(Endpoint::ConversationList, &asked);
        assert_eq!(query, vec![("category", "default".to_owned()), ("limit", "1".to_owned())]);
        let call = Call { endpoint: Endpoint::ConversationList, query, body: None };
        assert_eq!(call.url("http://127.0.0.1:9"), "http://127.0.0.1:9/fc/conversation-list?category=default&limit=1");
        // A conversation's id, and nothing a URL could be bent by.
        let asked = pairs(&[("conversationId", "a b&c=d"), ("groupId", "g")]);
        let call =
            Call { endpoint: Endpoint::Conversation, query: relay_query(Endpoint::Conversation, &asked), body: None };
        assert_eq!(call.url(API), "https://api.farcaster.xyz/fc/conversation?conversationId=a%20b%26c%3Dd");
        let long = "c".repeat(MAX_ID_LEN + 1);
        assert!(relay_query(Endpoint::MessageList, &pairs(&[("conversationId", &long)])).is_empty());
        assert!(relay_query(Endpoint::MessageList, &pairs(&[("cursor", "a\nb")])).is_empty());
        assert!(relay_query(Endpoint::Message, &pairs(&[("conversationId", "c")])).is_empty());
    }

    #[test]
    fn the_relay_sends_a_message_as_it_checked_it() {
        // Only the fields a send takes, the message trimmed.
        let body = r#"{"conversationId":"636f","message":"  hi \n","extra":{"x":1},"recipientFids":[1,2]}"#;
        assert_eq!(check_send(body).as_deref(), Ok(r#"{"conversationId":"636f","message":"hi"}"#));
        let reply = r#"{"groupId":"9f82","message":"gm","inReplyToMessageId":"m1"}"#;
        assert_eq!(check_send(reply).as_deref(), Ok(reply));
        assert_eq!(
            check_send(r#"{"recipientFid":6841,"message":"gm"}"#).as_deref(),
            Ok(r#"{"recipientFid":6841,"message":"gm"}"#)
        );
        // One target, no more, no fewer.
        assert_eq!(check_send(r#"{"message":"to no one"}"#), Err(SendProblem::Target));
        assert_eq!(check_send(r#"{"conversationId":"c","groupId":"g","message":"gm"}"#), Err(SendProblem::Target));
        // Ids that can't be.
        assert_eq!(check_send(r#"{"recipientFid":0,"message":"gm"}"#), Err(SendProblem::Id));
        assert_eq!(check_send(r#"{"conversationId":"","message":"gm"}"#), Err(SendProblem::Id));
        let long = format!(r#"{{"groupId":"{}","message":"gm"}}"#, "g".repeat(MAX_ID_LEN + 1));
        assert_eq!(check_send(&long), Err(SendProblem::Id));
        // A message that fits.
        assert_eq!(check_send(r#"{"conversationId":"c","message":"  "}"#), Err(SendProblem::Text(TextProblem::Empty)));
        let over = format!(r#"{{"conversationId":"c","message":"{}"}}"#, "a".repeat(MAX_CHARS + 1));
        assert_eq!(check_send(&over), Err(SendProblem::Text(TextProblem::TooLong { length: MAX_CHARS + 1 })));
        // Not a send at all.
        for bad in ["", "null", "[]", r#"{"conversationId":"c"}"#, r#"{"recipientFid":"42","message":"gm"}"#] {
            assert_eq!(check_send(bad), Err(SendProblem::Shape), "{bad:?}");
        }
        assert_eq!(
            check_send(r#"{"conversationId":"c","conversationId":"d","message":"gm"}"#),
            Err(SendProblem::Shape)
        );
        // The longest message, escaped, fits in a body.
        let longest =
            serde_json::to_string(&Send::to_thread(&Thread::Group("g".repeat(MAX_ID_LEN)), &"\u{1}".repeat(MAX_CHARS)))
                .unwrap();
        assert!(longest.len() <= MAX_BODY, "{}", longest.len());
    }

    #[test]
    fn idempotency_keys_are_plain() {
        assert!(check_idempotency("8f1c2e9a-7c1d-4a59-9d4e-3b1e6f0a2c11"));
        assert!(check_idempotency("a_B-9"));
        for bad in ["", "not one, x", "a b", "a\r\nb", "é", &"a".repeat(65)] {
            assert!(!check_idempotency(bad), "{bad:?}");
        }
    }
}

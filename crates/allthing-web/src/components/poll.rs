// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's poll on the front page (`allthing_core::poll`): one card in
//! two places, beside the feed where the screen has room for it
//! ([`PollGutter`]) and after the feed's fourth card where it hasn't
//! ([`inline`]); the stylesheet shows one of them. Before the reader votes
//! it asks, with no numbers. Once they have, it shows how many voted each
//! way, and says so to screen readers. Their vote, and the counts it was
//! answered with, are kept in this browser (`allthing-poll`), so a later
//! visit shows the results without asking the Worker, and the front page's
//! payload brings fresher counts. Where the Worker can't keep votes, it
//! hides itself. The static copy has none.
//!
//! The card's frame, with its question and a body of one height, is drawn
//! with the feed's cards. What fills the body (the buttons, the results,
//! voting, and reading and keeping the vote) is code of its own
//! (`split_poll.wasm`, [`body`]), fetched once the page is idle: until it
//! comes the body is empty, and nothing moves when it does.

use allthing_core::format::full_number;
use allthing_core::poll::{
    ALREADY, Answer, Choice, Counts, KEPT, Kept, MOBILE_CLIENT, NOT_SET_UP, POLL_PATH, QUESTION, Vote,
};
use leptos::prelude::*;

use crate::api::{self, Failure};

#[derive(Clone, Debug)]
enum State {
    /// The poll's code hasn't come yet, which reads the reader's vote.
    Unread,
    /// Not voted: Yes and No, and why the last vote didn't go through.
    Open(Option<String>),
    /// A vote sent, waiting for the Worker's answer.
    Voting(Choice),
    /// The results, as the reader last saw them.
    Voted(Kept),
    /// The Worker can't keep votes here.
    Hidden,
}

/// The poll, shared by its two places.
#[derive(Clone, Copy)]
pub struct Poll {
    state: RwSignal<State>,
    /// What screen readers are told: the results once a vote is answered,
    /// or why it didn't go through.
    said: RwSignal<String>,
    /// The counts a front page's payload brought last, and when they were
    /// counted (Unix milliseconds).
    offered: RwSignal<Option<(Counts, f64)>>,
}

impl Poll {
    pub fn new() -> Poll {
        Poll { state: RwSignal::new(State::Unread), said: RwSignal::new(String::new()), offered: RwSignal::new(None) }
    }

    /// Counts a front page's payload brought, counted at `at` (Unix
    /// milliseconds): shown in place of those the reader saw, if they voted
    /// and these are newer.
    pub fn offer(self, counts: Counts, at: f64) {
        self.offered.set(Some((counts, at)));
    }
}

impl Default for Poll {
    fn default() -> Poll {
        Poll::new()
    }
}

/// The poll beside the feed, where the screen has room for it: it stays in
/// view as the feed scrolls.
#[component]
pub fn PollGutter() -> impl IntoView {
    view! {
        <div class="poll-gutter">
            <PollCard place="side"/>
        </div>
    }
}

/// The poll as a card in the feed, after its fourth (`CastList`'s inset).
pub fn inline() -> AnyView {
    view! { <PollCard place="inline"/> }.into_any()
}

thread_local! {
    /// Whether the poll's code has come: then a card's body fills at once.
    static CAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[component]
fn PollCard(place: &'static str) -> impl IntoView {
    let Some(poll) = use_context::<Poll>() else { return ().into_any() };
    let question = format!("poll-question-{place}");
    let shown = move || poll.state.with(|state| !matches!(state, State::Hidden));
    let total = move || {
        poll.state.with(|state| match state {
            State::Voted(kept) => kept.counts.votes(),
            _ => String::new(),
        })
    };
    // Its code once the page is idle, the first time: the feed's cards and
    // their images come first.
    let filled = move || {
        crate::split::lazy_extra(|| (), async move {
            if !CAME.with(std::cell::Cell::get) {
                idle().await;
            }
            let view = body(poll).await;
            CAME.with(|came| came.set(view.is_ok()));
            view
        })
    };
    view! {
        <Show when=shown>
            <section class=format!("poll-card poll-{place}") aria-labelledby=question.clone()>
                <div class="poll-head">
                    <p class="poll-overline">"Allthing poll"</p>
                    <p class="poll-total">{total}</p>
                </div>
                <h2 class="poll-question" id=question.clone()>{QUESTION}</h2>
                <div class="poll-body">{filled()}</div>
                <p class="visually-hidden" role="status">{move || poll.said.get()}</p>
            </section>
        </Show>
    }
    .into_any()
}

/// Once the browser is idle (or soon, where it can't say).
async fn idle() {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        crate::components::cast::idle(move || {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        });
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// What fills a card's body, its code fetched the first time: the reader's
/// vote read from this browser (once, for both places), the counts the
/// front page's payload brings, and Yes and No or the results.
#[leptos::wasm_split::wasm_split(poll, wasm_split_path = ::leptos::wasm_split, fallible)]
fn body(poll: Poll) -> Result<AnyView, leptos::wasm_split::SplitLoaderError> {
    if poll.state.with_untracked(|state| matches!(state, State::Unread)) {
        poll.state.set(read().map_or(State::Open(None), State::Voted));
    }
    Effect::new(move |_| {
        if let Some((counts, at)) = poll.offered.get() {
            take_newer(poll, counts, at);
        }
    });
    Ok(view! { <Body poll/> }.into_any())
}

/// Counts a front page's payload brought, counted at `at`, in place of
/// those the reader saw if they are newer. Nothing before the reader has
/// voted.
fn take_newer(poll: Poll, counts: Counts, at: f64) {
    let newer = poll.state.with_untracked(|state| match state {
        State::Voted(kept) if at > kept.at => Some(kept.clone().newer(counts, at)),
        _ => None,
    });
    if let Some(kept) = newer {
        write(&kept);
        poll.state.set(State::Voted(kept));
    }
}

fn vote(poll: Poll, choice: Choice) {
    if !poll.state.with_untracked(|state| matches!(state, State::Open(_))) {
        return;
    }
    poll.state.set(State::Voting(choice));
    poll.said.set(String::new());
    wasm_bindgen_futures::spawn_local(async move {
        let vote = Vote { poll: MOBILE_CLIENT.to_owned(), choice };
        let next = match api::post::<Vote, Answer>(POLL_PATH, &vote).await {
            Ok(answer) => {
                // A network that had voted is told so, never how.
                let choice = answer.counted.then_some(choice);
                let kept =
                    Kept { poll: MOBILE_CLIENT.to_owned(), choice, counts: answer.counts, at: js_sys::Date::now() };
                write(&kept);
                let _ = poll.said.try_set(kept.counts.summary(choice));
                State::Voted(kept)
            }
            Err(Failure::Busy { message, .. }) if message == NOT_SET_UP => State::Hidden,
            Err(failure) => {
                let why = reason(&failure);
                let _ = poll.said.try_set(why.clone());
                State::Open(Some(why))
            }
        };
        let _ = poll.state.try_set(next);
    });
}

/// Why a vote didn't go through, in two lines of the card at most.
fn reason(failure: &Failure) -> String {
    match failure {
        Failure::Busy { message, .. } if !message.is_empty() && message.len() <= 60 => message.clone(),
        failure if failure.is_transient() => "Allthing couldn't be reached. Try again.".to_owned(),
        _ => "Your vote didn't go through. Try again.".to_owned(),
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// The reader's vote in this poll, as this browser kept it.
fn read() -> Option<Kept> {
    let kept = storage()?.get_item(KEPT).ok().flatten()?;
    serde_json::from_str::<Kept>(&kept).ok().filter(|kept| kept.poll == MOBILE_CLIENT)
}

fn write(kept: &Kept) {
    if let (Some(storage), Ok(json)) = (storage(), serde_json::to_string(kept)) {
        let _ = storage.set_item(KEPT, &json);
    }
}

/// Yes and No, or the results once the reader has voted.
#[component]
fn Body(poll: Poll) -> impl IntoView {
    let voted = Memo::new(move |_| poll.state.with(|state| matches!(state, State::Voted(_))));
    move || if voted.get() { view! { <Results poll/> }.into_any() } else { view! { <Ask poll/> }.into_any() }
}

/// Yes and No: the one chosen shows pressed, and both wait, until the
/// Worker answers.
#[component]
fn Ask(poll: Poll) -> impl IntoView {
    let voting = move || {
        poll.state.with(|state| match state {
            State::Voting(choice) => Some(*choice),
            _ => None,
        })
    };
    let why = move || {
        poll.state.with(|state| match state {
            State::Open(why) => why.clone(),
            _ => None,
        })
    };
    view! {
        <div class="poll-choices">
            {Choice::ALL
                .into_iter()
                .map(|choice| {
                    view! {
                        <button
                            type="button"
                            class="poll-choice"
                            aria-pressed=move || (voting() == Some(choice)).to_string()
                            disabled=move || voting().is_some()
                            on:click=move |_| vote(poll, choice)
                        >
                            {choice.label()}
                        </button>
                    }
                })
                .collect_view()}
        </div>
        <p class="poll-note" class:failed=move || why().is_some()>
            {move || why().unwrap_or_else(|| "See the results once you vote.".to_owned())}
        </p>
    }
}

/// How many voted each way, and the reader's vote: theirs, or that their
/// network's was counted already.
#[component]
fn Results(poll: Poll) -> impl IntoView {
    move || {
        poll.state.with(|state| {
            let State::Voted(kept) = state else { return ().into_any() };
            let counts = kept.counts;
            let mine = kept.choice;
            let rows = Choice::ALL
                .into_iter()
                .map(|choice| {
                    let share = counts.percent(choice);
                    view! {
                        <li class="poll-result" class:mine={mine == Some(choice)} style=format!("--share: {share}%")>
                            <span class="poll-label">{choice.label()}</span>
                            " "
                            <span class="poll-share">{format!("{share}% ({})", full_number(counts.of(choice)))}</span>
                        </li>
                    }
                })
                .collect_view();
            let whose = match mine {
                Some(choice) => format!("You voted {}.", choice.label()),
                None => ALREADY.to_owned(),
            };
            view! {
                <ul class="poll-results">{rows}</ul>
                <p class="poll-note">{whose}</p>
            }
            .into_any()
        })
    }
}

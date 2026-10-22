// SPDX-License-Identifier: AGPL-3.0-or-later
//! Scheduled casts going out (see `allthing_core::scheduled`). Each minute,
//! in the indexer's cron run, the casts whose time has come are taken
//! (`sending`, out of reach of a cancel or a change), given to the node as
//! their readers' browsers signed them, and what the node says is kept for
//! the readers' lists: out, or why not. A node that can't take one just now
//! is offered it again the next minute, up to
//! [`MAX_TRIES`](allthing_core::scheduled::MAX_TRIES) times; one the run
//! didn't try waits for the next run as it was.

use allthing_core::message::{self, Submitted};
use allthing_core::provenance::base64_decode;
use allthing_core::scheduled::{KEEP_DONE, MAX_TRIES};
use futures::future::LocalBoxFuture;

use crate::Invocation;
use crate::indexer::IndexError;
use crate::upstream::{self, UpstreamError};

/// Casts given to the node in one run, at most: one subrequest each, between
/// the read of what is due and the write of what became of it.
pub const PER_RUN: u32 = 5;

const KEY_GONE: &str = "The key it was signed with isn't on your Farcaster account anymore. Edit it to sign it again.";
const UNREADABLE: &str = "Its signed message couldn't be read.";
const NO_ANSWER: &str = "Farcaster's network didn't take it for half an hour. Edit it to try again.";

/// How long a run's hold on the casts it took lasts: a run that ended
/// before it said what became of them (out of time, or its write failed)
/// leaves them `sending`, and the run that comes this long after takes them
/// again. A node given one twice says it has it already.
pub const HELD: i64 = 300;

/// Where scheduled casts are kept (D1, in the Worker).
pub trait ScheduledStore {
    /// Take up to `limit` casts waiting whose time is `now` (Unix seconds) or
    /// before, the earliest first, and any taken before `now -` [`HELD`] and
    /// never settled: each is `sending` from `now`, so a cancel or a change
    /// that comes while the node is given it is told it's too late, until
    /// [`settle`](ScheduledStore::settle) says what became of it.
    fn claim_due(&self, now: i64, limit: u32) -> LocalBoxFuture<'static, Result<Vec<Due>, IndexError>>;
    /// Keep what became of them, at `now`; and drop casts that went out, or
    /// didn't, longer than the list keeps them.
    fn settle(&self, settled: Vec<Settled>, now: i64) -> LocalBoxFuture<'static, Result<(), IndexError>>;
}

/// A cast whose time has come.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    pub id: i64,
    /// Its hash: what became of it is kept only if the reader hasn't put
    /// another cast in its place meanwhile.
    pub hash: String,
    /// Its signed message, in base64.
    pub message: String,
    /// How many times a node couldn't take it.
    pub tries: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Posted,
    /// Given up, and why, for the reader.
    Failed(String),
    /// The node couldn't take it just now: again next minute.
    Again,
    /// Not given to the node this run (its breaker was open, its bucket
    /// empty, or the run's budget spent): waiting again, with no try
    /// counted.
    Unsent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settled {
    pub id: i64,
    pub hash: String,
    pub outcome: Outcome,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub due: usize,
    pub posted: usize,
    pub failed: usize,
    pub again: usize,
    pub unsent: usize,
}

/// How long the list keeps a cast that went out, or didn't: a store drops
/// those finished before `now` minus this.
pub fn kept_since(now: i64) -> i64 {
    now - KEEP_DONE
}

/// Casts a run took before this, at `now`, and never settled, are taken
/// again ([`HELD`]).
pub fn held_since(now: i64) -> i64 {
    now - HELD
}

fn d1(inv: &Invocation) -> Result<(), IndexError> {
    if inv.take_subrequests(1) { Ok(()) } else { Err(IndexError("no subrequests left for D1".into())) }
}

/// One run at `now` (Unix seconds): take the casts that are due, give them
/// to the node, and say what became of each, the ones it didn't try too.
/// Nothing is written when none are due.
pub async fn run(inv: &Invocation, store: &dyn ScheduledStore, now: i64) -> Result<Report, IndexError> {
    d1(inv)?;
    let due = store.claim_due(now, PER_RUN).await?;
    let mut report = Report { due: due.len(), ..Report::default() };
    if due.is_empty() {
        return Ok(report);
    }
    let mut settled = Vec::new();
    for cast in due {
        let outcome = match base64_decode(&cast.message) {
            None => Outcome::Failed(UNREADABLE.into()),
            // One subrequest stays for the write.
            Some(_) if inv.remaining_subrequests() < 2 => Outcome::Unsent,
            Some(bytes) => match upstream::submit(inv, &message::submit_url(), bytes).await {
                Ok((status, body)) => match message::submitted(status, &body) {
                    Submitted::Accepted | Submitted::Duplicate => Outcome::Posted,
                    Submitted::UnknownKey => Outcome::Failed(KEY_GONE.into()),
                    Submitted::Refused(why) if !why.is_empty() => {
                        Outcome::Failed(format!("Farcaster didn't take it: {why}"))
                    }
                    Submitted::Refused(_) | Submitted::Unavailable => again(cast.tries),
                },
                // Not tried: the node's breaker is open, or its bucket is empty.
                Err(UpstreamError::Busy(_) | UpstreamError::Budget) => Outcome::Unsent,
                Err(_) => again(cast.tries),
            },
        };
        match outcome {
            Outcome::Posted => report.posted += 1,
            Outcome::Failed(_) => report.failed += 1,
            Outcome::Again => report.again += 1,
            Outcome::Unsent => report.unsent += 1,
        }
        settled.push(Settled { id: cast.id, hash: cast.hash, outcome });
    }
    d1(inv)?;
    store.settle(settled, now).await?;
    Ok(report)
}

/// Again next minute, or, after the last try, given up.
fn again(tries: u32) -> Outcome {
    if tries + 1 >= MAX_TRIES { Outcome::Failed(NO_ANSWER.into()) } else { Outcome::Again }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use futures::FutureExt;

    use super::*;
    use crate::{FetchError, Host, HttpMethod, HttpRequest, HttpResponse};

    const NOW: i64 = 1_800_000_000;

    /// A request the node was given: its method, URL, octets, and whether
    /// any answer's body was asked for.
    type Given = (HttpMethod, String, Vec<u8>, bool);

    /// A node that answers each message in turn, and what it was given.
    #[derive(Default)]
    struct Node {
        answers: RefCell<VecDeque<Result<(u16, &'static str), FetchError>>>,
        given: RefCell<Vec<Given>>,
    }

    impl Host for Node {
        fn now(&self) -> f64 {
            NOW as f64 * 1000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            let octets = request.octets.clone().unwrap_or_default();
            self.given.borrow_mut().push((request.method, request.url, octets, request.any_body));
            let answer = self.answers.borrow_mut().pop_front().unwrap_or(Ok((200, "{}")));
            async move {
                answer.map(|(status, body)| HttpResponse {
                    status,
                    retry_after: None,
                    body: Some(body.as_bytes().to_vec()),
                })
            }
            .boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    /// D1, in memory.
    #[derive(Default)]
    struct Store {
        due: Vec<Due>,
        settled: RefCell<Vec<(Vec<Settled>, i64)>>,
        asked: RefCell<Vec<(i64, u32)>>,
    }

    impl ScheduledStore for Rc<Store> {
        fn claim_due(&self, now: i64, limit: u32) -> LocalBoxFuture<'static, Result<Vec<Due>, IndexError>> {
            self.asked.borrow_mut().push((now, limit));
            let due = self.due.iter().take(limit as usize).cloned().collect();
            async move { Ok(due) }.boxed_local()
        }

        fn settle(&self, settled: Vec<Settled>, now: i64) -> LocalBoxFuture<'static, Result<(), IndexError>> {
            self.settled.borrow_mut().push((settled, now));
            async move { Ok(()) }.boxed_local()
        }
    }

    fn due(id: i64, message: &str, tries: u32) -> Due {
        Due { id, hash: format!("0x{id:040x}"), message: message.into(), tries }
    }

    fn setup(
        due: Vec<Due>,
        answers: Vec<Result<(u16, &'static str), FetchError>>,
        budget: u32,
    ) -> (Rc<Node>, Rc<Store>, Invocation) {
        let node = Rc::new(Node { answers: RefCell::new(answers.into()), ..Node::default() });
        let store = Rc::new(Store { due, ..Store::default() });
        let inv = Invocation::new(node.clone(), budget);
        (node, store, inv)
    }

    #[test]
    fn due_casts_go_to_the_node_as_signed_and_what_it_says_is_kept() {
        let refused =
            r#"{"error":"Failed to submit","error_detail":"bad_request.validation_failure/text is too long"}"#;
        let unknown = r#"{"error":"Failed","error_detail":"bad_request.validation_failure/invalid signer"}"#;
        let duplicate = r#"{"error":"Failed","error_detail":"bad_request.duplicate"}"#;
        let (node, store, inv) = setup(
            vec![due(1, "AQID", 0), due(2, "BAUG", 0), due(3, "BwgJ", 0), due(4, "CgsM", 0)],
            vec![Ok((200, "{}")), Ok((400, refused)), Ok((400, unknown)), Ok((400, duplicate))],
            45,
        );
        let report = run(&inv, &store, NOW).now_or_never().unwrap().unwrap();
        assert_eq!(report, Report { due: 4, posted: 2, failed: 2, ..Report::default() });
        assert_eq!(*store.asked.borrow(), [(NOW, PER_RUN)]);
        // The bytes the browser signed, as octets, to the node's submitMessage.
        let given = node.given.borrow();
        assert_eq!(given.len(), 4);
        assert_eq!(
            given[0],
            (HttpMethod::Post, "https://haatz.quilibrium.com/v1/submitMessage".into(), vec![1, 2, 3], true)
        );
        assert_eq!(given[1].2, vec![4, 5, 6]);
        let settled = store.settled.borrow();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].1, NOW);
        let outcomes: Vec<_> = settled[0].0.iter().map(|s| (s.id, s.outcome.clone())).collect();
        assert_eq!(settled[0].0[2].hash, format!("0x{:040x}", 3));
        assert_eq!(
            outcomes,
            [
                (1, Outcome::Posted),
                (2, Outcome::Failed("Farcaster didn't take it: text is too long".into())),
                (3, Outcome::Failed(KEY_GONE.into())),
                (4, Outcome::Posted),
            ]
        );
        // The run took a read, four submits and a write.
        assert_eq!(inv.remaining_subrequests(), 45 - 6);
    }

    #[test]
    fn a_node_that_cant_take_one_now_gets_it_again_until_the_last_try() {
        let (_, store, inv) = setup(
            vec![due(1, "AQID", 0), due(2, "BAUG", MAX_TRIES - 1), due(3, "BwgJ", 3)],
            vec![Ok((503, "")), Ok((503, "")), Err(FetchError::Timeout)],
            45,
        );
        let report = run(&inv, &store, NOW).now_or_never().unwrap().unwrap();
        assert_eq!(report, Report { due: 3, failed: 1, again: 2, ..Report::default() });
        let settled = store.settled.borrow();
        let outcomes: Vec<_> = settled[0].0.iter().map(|s| s.outcome.clone()).collect();
        assert_eq!(outcomes, [Outcome::Again, Outcome::Failed(NO_ANSWER.into()), Outcome::Again]);
    }

    #[test]
    fn nothing_due_costs_one_read_and_no_write() {
        let (node, store, inv) = setup(vec![], vec![], 45);
        let report = run(&inv, &store, NOW).now_or_never().unwrap().unwrap();
        assert_eq!(report, Report::default());
        assert!(node.given.borrow().is_empty() && store.settled.borrow().is_empty());
        assert_eq!(inv.remaining_subrequests(), 44);
    }

    #[test]
    fn a_short_budget_leaves_the_rest_for_the_next_run_and_keeps_the_write() {
        let (node, store, inv) = setup(vec![due(1, "AQID", 0), due(2, "BAUG", 0), due(3, "BwgJ", 0)], vec![], 4);
        let report = run(&inv, &store, NOW).now_or_never().unwrap().unwrap();
        // A read, two submits, and the write.
        assert_eq!(report, Report { due: 3, posted: 2, unsent: 1, ..Report::default() });
        assert_eq!(node.given.borrow().len(), 2);
        assert_eq!(inv.remaining_subrequests(), 0);
        // The one it couldn't give the node waits for the next run, as it
        // was: no longer held, with no try counted.
        let settled = store.settled.borrow();
        let outcomes: Vec<_> = settled[0].0.iter().map(|s| (s.id, s.outcome.clone())).collect();
        assert_eq!(outcomes, [(1, Outcome::Posted), (2, Outcome::Posted), (3, Outcome::Unsent)]);
    }

    #[test]
    fn once_the_nodes_breaker_opens_the_rest_wait_untried() {
        // Three failures in a row open the breaker: the casts after them
        // aren't given to the node, and wait as they were.
        let down = || Err(FetchError::Timeout);
        let (node, store, inv) = setup(
            vec![due(1, "AQID", 0), due(2, "BAUG", 0), due(3, "BwgJ", 0), due(4, "CgsM", 2), due(5, "DQ4P", 0)],
            vec![down(), down(), down()],
            45,
        );
        let report = run(&inv, &store, NOW).now_or_never().unwrap().unwrap();
        assert_eq!(report, Report { due: 5, again: 3, unsent: 2, ..Report::default() });
        assert_eq!(node.given.borrow().len(), 3);
        let settled = store.settled.borrow();
        let outcomes: Vec<_> = settled[0].0.iter().map(|s| (s.id, s.outcome.clone())).collect();
        assert_eq!(
            outcomes,
            [(1, Outcome::Again), (2, Outcome::Again), (3, Outcome::Again), (4, Outcome::Unsent), (5, Outcome::Unsent)]
        );
        // A read, three submits and the write.
        assert_eq!(inv.remaining_subrequests(), 45 - 5);
    }

    #[test]
    fn an_unreadable_message_is_given_up_without_a_request() {
        let (node, store, inv) = setup(vec![due(1, "not base64!", 0)], vec![], 45);
        let report = run(&inv, &store, NOW).now_or_never().unwrap().unwrap();
        assert_eq!(report, Report { due: 1, failed: 1, ..Report::default() });
        assert!(node.given.borrow().is_empty());
        assert_eq!(store.settled.borrow()[0].0[0].outcome, Outcome::Failed(UNREADABLE.into()));
        assert_eq!(kept_since(NOW), NOW - KEEP_DONE);
        assert_eq!(held_since(NOW), NOW - HELD);
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! Which links open a Farcaster mini app or frame. Hypersnap gives no embed
//! metadata, and mini apps on their own domains can't be told by URL, so the
//! linked page's own meta tags are read: a few unknown links per build, with
//! verdicts remembered per isolate and carried in each payload.

use std::cell::RefCell;
use std::collections::BTreeMap;

use allthing_core::embeds::{html_declares_mini_app, link_key, worth_checking_for_mini_app};
use allthing_core::model::Cast;
use futures::future::join_all;

use crate::annotate::Bounded;
use crate::upstream::{self, Upstream, UpstreamError};
use crate::{HttpMethod, Invocation};

/// Link pages fetched while building one payload, at most; the rest are
/// checked on a later refresh. One batch of [`PARALLEL`]: a second batch
/// waited for the first's slowest page, and doubled a cold build's wait.
pub const MAX_CHECKS: usize = PARALLEL;
/// Subrequests left for the rest of the build (origins, app names, the cache
/// write): checks never take these.
const RESERVE: usize = 10;
/// A Worker keeps at most six connections open at once, so pages are read in
/// batches of six: a queued fetch would spend its timeout waiting.
const PARALLEL: usize = 6;
/// How long a verdict is trusted.
const VERDICT_TTL_MS: f64 = 86_400_000.0;
/// How long a page that couldn't be read is left alone before trying again.
const RETRY_MS: f64 = 3_600_000.0;

thread_local! {
    /// link key -> (until, Some(is a mini app), or None: couldn't be read).
    static VERDICTS: RefCell<Bounded<String, (f64, Option<bool>)>> = RefCell::new(Bounded::new(20_000));
}

fn remembered(key: &str, now: f64) -> Option<Option<bool>> {
    VERDICTS.with(|v| v.borrow().get(&key.to_owned()).filter(|(until, _)| now < *until).map(|(_, verdict)| *verdict))
}

fn remember(key: String, now: f64, verdict: Option<bool>) {
    let until = now + if verdict.is_some() { VERDICT_TTL_MS } else { RETRY_MS };
    VERDICTS.with(|v| v.borrow_mut().insert(key, (until, verdict)));
}

/// Read one page, leaving locally deferred checks eligible for the next build.
async fn read(inv: &Invocation, url: &str) -> Result<Option<bool>, UpstreamError> {
    Ok(match upstream::send(inv, Upstream::LinkPage, HttpMethod::Get, url, None, false).await {
        Ok(Some(bytes)) => Some(html_declares_mini_app(&String::from_utf8_lossy(&bytes))),
        // Gone or refused: there's no mini app to run there.
        Ok(None) | Err(UpstreamError::Status(400..=499)) => Some(false),
        Err(error @ (UpstreamError::Budget | UpstreamError::Busy(_))) => return Err(error),
        Err(_) => None,
    })
}

/// Verdicts for the links of `casts` that aren't already hidden (known mini
/// apps, spam): `known` (carried from the previous payload) first, then this
/// isolate's memory, then up to [`MAX_CHECKS`] pages read now, as the build's
/// budget allows. Links still
/// unknown are left out, and their casts stay listed until they are known.
pub async fn check<'a>(
    inv: &Invocation,
    casts: impl IntoIterator<Item = &'a Cast>,
    known: &BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    let now = inv.now();
    let mut verdicts = BTreeMap::new();
    let mut unknown = Vec::new();
    for url in casts.into_iter().filter(|c| !c.hidden()).flat_map(Cast::link_urls) {
        if !worth_checking_for_mini_app(url) {
            continue;
        }
        let key = link_key(url).to_owned();
        if verdicts.contains_key(&key) || unknown.contains(&key) {
            continue;
        }
        match known.get(&key).copied().map(Some).or_else(|| remembered(&key, now)) {
            Some(Some(verdict)) => {
                verdicts.insert(key, verdict);
            }
            // Couldn't be read a moment ago: try again later.
            Some(None) => {}
            None => unknown.push(key),
        }
    }
    let affordable = (inv.remaining_subrequests() as usize).saturating_sub(RESERVE);
    unknown.truncate(inv.take_link_checks(unknown.len().min(affordable)));
    for batch in unknown.chunks(PARALLEL) {
        let read = join_all(batch.iter().map(|url| read(inv, url))).await;
        for (key, verdict) in batch.iter().zip(read) {
            let Ok(verdict) = verdict else { continue };
            remember(key.clone(), now, verdict);
            if let Some(verdict) = verdict {
                verdicts.insert(key.clone(), verdict);
            }
        }
    }
    verdicts
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use allthing_core::model::{Embed, UserSummary};
    use futures::future::{FutureExt, LocalBoxFuture};

    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};

    /// Serves a mini app at gig-bot, a plain page at example.com, a 404 at
    /// gone.example, and nothing anywhere else.
    struct Pages {
        requests: RefCell<Vec<String>>,
    }

    impl Host for Pages {
        fn now(&self) -> f64 {
            1000.0
        }

        fn fetch(&self, request: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            assert_eq!(request.accept, "text/html");
            assert!(request.max_bytes.is_some());
            self.requests.borrow_mut().push(request.url.clone());
            let page = |html: &str| Ok(HttpResponse { status: 200, retry_after: None, body: Some(html.into()) });
            let response = if request.url.starts_with("https://gig-bot-test.vercel.app") {
                page(r#"<html><head><meta name="fc:miniapp" content="{}"></head></html>"#)
            } else if request.url.starts_with("https://example.com") {
                page("<html><head><title>A post</title></head></html>")
            } else if request.url.starts_with("https://gone.example") {
                Ok(HttpResponse { status: 404, retry_after: None, body: None })
            } else {
                Err(FetchError::Network("unreachable".into()))
            };
            async move { response }.boxed_local()
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {}
    }

    fn linking(url: &str) -> Cast {
        Cast {
            hash: url.into(),
            author: UserSummary { fid: 1, username: "a".into(), display_name: String::new(), pfp_url: None },
            text: String::new(),
            mentions: vec![],
            ts: 0,
            parent_hash: None,
            parent_url: None,
            channel: None,
            embeds: vec![Embed::Link { url: url.into(), image_url: None, title: None }],
            likes: 0,
            recasts: 0,
            replies: 0,
            mini_app: false,
            references: Default::default(),
            spam: false,
        }
    }

    #[test]
    fn reads_pages_once_and_remembers_what_they_are() {
        let host = Rc::new(Pages { requests: RefCell::new(vec![]) });
        let inv = Invocation::new(host.clone(), 45);
        let casts = [
            linking("https://gig-bot-test.vercel.app/#play"),
            linking("https://example.com/a-post"),
            linking("https://gone.example/x"),
            linking("https://unreachable.example/"),
            linking("https://www.youtube.com/watch?v=1"),
        ];
        let verdicts = check(&inv, &casts, &BTreeMap::new()).now_or_never().unwrap();
        assert_eq!(
            verdicts,
            BTreeMap::from([
                ("https://gig-bot-test.vercel.app/".to_owned(), true),
                ("https://example.com/a-post".to_owned(), false),
                ("https://gone.example/x".to_owned(), false),
            ])
        );
        // YouTube is never fetched; the unreachable page isn't reported.
        assert_eq!(host.requests.borrow().len(), 4);

        // Next build: everything is remembered, nothing is fetched again.
        let again = check(&inv, &casts, &BTreeMap::new()).now_or_never().unwrap();
        assert_eq!(again, verdicts);
        assert_eq!(host.requests.borrow().len(), 4);
    }

    #[test]
    fn carried_verdicts_need_no_fetch_and_checks_are_capped() {
        let host = Rc::new(Pages { requests: RefCell::new(vec![]) });
        let inv = Invocation::new(host.clone(), 45);
        let carried = BTreeMap::from([("https://carried.example/".to_owned(), true)]);
        let mut casts = vec![linking("https://carried.example/")];
        casts.extend((0..20).map(|i| linking(&format!("https://example.com/capped-{i}"))));
        let verdicts = check(&inv, &casts, &carried).now_or_never().unwrap();
        assert_eq!(verdicts.get("https://carried.example/"), Some(&true));
        assert_eq!(host.requests.borrow().len(), MAX_CHECKS);
        assert_eq!(verdicts.len(), 1 + MAX_CHECKS);
    }

    #[test]
    fn a_build_with_little_budget_left_keeps_its_reserve() {
        let host = Rc::new(Pages { requests: RefCell::new(vec![]) });
        let inv = Invocation::new(host.clone(), 13);
        let casts: Vec<Cast> = (0..20).map(|i| linking(&format!("https://example.com/tight-{i}"))).collect();
        check(&inv, &casts, &BTreeMap::new()).now_or_never().unwrap();
        assert_eq!(host.requests.borrow().len(), 13 - RESERVE);
    }
}

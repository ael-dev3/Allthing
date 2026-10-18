// SPDX-License-Identifier: AGPL-3.0-or-later
//! Scores, cast origins and app names for the casts on a page.
//!
//! Each comes from three places, cheapest first: the previous payload for the
//! same page (carried forward), bounded in-isolate memory, and finally a
//! strictly limited number of upstream requests. Whatever cannot be resolved
//! within the limit is counted as `pending` and picked up by later refreshes.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::Hash;
use std::rc::Rc;

use allthing_core::api::Annotations;
use allthing_core::model::UserSummary;
use allthing_core::normalize;
use allthing_core::provenance::Origin;
use futures::future::{join, join_all};
use serde::Deserialize;

use crate::Invocation;
use crate::{chain, hypersnap};

/// A map that forgets its oldest insertions beyond a capacity.
pub(crate) struct Bounded<K, V> {
    map: HashMap<K, V>,
    order: VecDeque<K>,
    capacity: usize,
}

impl<K: Eq + Hash + Clone, V> Bounded<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Bounded { map: HashMap::new(), order: VecDeque::new(), capacity }
    }

    pub(crate) fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key)
    }

    pub(crate) fn insert(&mut self, key: K, value: V) {
        if self.map.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
            while self.order.len() > self.capacity {
                if let Some(old) = self.order.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
    }
}

type SignerApps = Rc<HashMap<String, Option<u64>>>;

const SIGNERS_TTL_MS: f64 = 3_600_000.0;
const APPS_TTL_MS: f64 = 86_400_000.0;
const SCORES_TTL_MS: f64 = 43_200_000.0;

thread_local! {
    /// Cast hash -> origin. Origins never change, so no TTL.
    static ORIGINS: RefCell<Bounded<String, Origin>> = RefCell::new(Bounded::new(20_000));
    /// fid -> (fetched, active signer key -> app fid).
    static SIGNERS: RefCell<Bounded<u64, (f64, SignerApps)>> = RefCell::new(Bounded::new(4_000));
    /// app fid -> (fetched, app account).
    static APPS: RefCell<Bounded<u64, (f64, UserSummary)>> = RefCell::new(Bounded::new(2_000));
    /// fid -> (fetched, Neynar score in millionths).
    static SCORES: RefCell<Bounded<u64, (f64, u32)>> = RefCell::new(Bounded::new(50_000));
}

/// The annotations of a previously cached payload for the same page.
pub fn carried(previous: Option<&[u8]>) -> Annotations {
    #[derive(Deserialize)]
    struct Previous {
        #[serde(default)]
        notes: Annotations,
    }
    previous.and_then(|bytes| serde_json::from_slice::<Previous>(bytes).ok()).map(|p| p.notes).unwrap_or_default()
}

/// A cast to attribute: (author fid, cast hash), in display priority order.
pub type Target = (u64, String);

pub fn remember_scores(inv: &Invocation, scores: &HashMap<u64, u32>) {
    let now = inv.now();
    SCORES.with(|s| {
        let mut s = s.borrow_mut();
        for (fid, score) in scores {
            s.insert(*fid, (now, *score));
        }
    });
}

/// Neynar scores for `fids`, from memory or one batched onchain read.
pub async fn scores(inv: &Invocation, fids: &[u64]) -> BTreeMap<u64, u32> {
    let now = inv.now();
    let mut out = BTreeMap::new();
    let mut missing = Vec::new();
    SCORES.with(|s| {
        let s = s.borrow();
        for fid in fids {
            match s.get(fid) {
                Some((at, score)) if now - at < SCORES_TTL_MS => {
                    out.insert(*fid, *score);
                }
                _ => missing.push(*fid),
            }
        }
    });
    missing.sort_unstable();
    missing.dedup();
    if !missing.is_empty()
        && let Ok(fetched) = chain::neynar_scores(inv, &missing).await
    {
        remember_scores(inv, &fetched);
        out.extend(fetched);
    }
    out
}

async fn signer_apps(inv: &Invocation, fid: u64) -> Option<SignerApps> {
    let now = inv.now();
    let cached = SIGNERS
        .with(|s| s.borrow().get(&fid).filter(|(at, _)| now - at < SIGNERS_TTL_MS).map(|(_, apps)| apps.clone()));
    if cached.is_some() {
        return cached;
    }
    let apps = Rc::new(hypersnap::active_signers(inv, fid).await.ok()?);
    SIGNERS.with(|s| s.borrow_mut().insert(fid, (now, apps.clone())));
    Some(apps)
}

pub fn remember_signers(inv: &Invocation, fid: u64, apps: HashMap<String, Option<u64>>) -> SignerApps {
    let apps = Rc::new(apps);
    SIGNERS.with(|s| s.borrow_mut().insert(fid, (inv.now(), apps.clone())));
    apps
}

/// Attribute an author's casts given each cast's signer, falling back to the
/// full onchain history for keys that have since been removed.
pub async fn attribute(
    inv: &Invocation,
    fid: u64,
    cast_signers: &[(String, String)],
    active: Option<SignerApps>,
) -> Vec<(String, Origin)> {
    let mut apps = active.map(|a| a.as_ref().clone()).unwrap_or_default();
    // Whether every key the account ever registered is in `apps`.
    let mut complete = false;
    if cast_signers.iter().any(|(_, signer)| !apps.contains_key(signer))
        && let Ok(history) = hypersnap::all_onchain_signers(inv, fid).await
    {
        for (key, app) in history {
            apps.entry(key).or_insert(app);
        }
        complete = true;
    }
    let out: Vec<(String, Origin)> = cast_signers
        .iter()
        .filter_map(|(hash, signer)| {
            // A key missing because the history couldn't be read (an error,
            // or no budget left) is not known yet: leave the cast to retry
            // instead of recording "no app" for good.
            let app_fid = match apps.get(signer) {
                Some(app) => *app,
                None if complete => None,
                None => return None,
            };
            Some((hash.clone(), Origin { signer: signer.clone(), app_fid }))
        })
        .collect();
    ORIGINS.with(|o| {
        let mut o = o.borrow_mut();
        for (hash, origin) in &out {
            o.insert(hash.clone(), origin.clone());
        }
    });
    out
}

/// Resolve one author's casts: their signers' apps plus each cast's signer
/// (one `castById`, or one `castsByFid` page for several casts).
async fn resolve_author(inv: &Invocation, fid: u64, hashes: Vec<String>) -> Vec<(String, Origin)> {
    let cast_signers = async {
        if let [hash] = hashes.as_slice() {
            hypersnap::cast_signer(inv, fid, hash)
                .await
                .ok()
                .flatten()
                .map(|signer| vec![(hash.clone(), signer)])
                .unwrap_or_default()
        } else {
            let recent = hypersnap::recent_cast_signers(inv, fid, 100).await.unwrap_or_default();
            hashes.iter().filter_map(|h| recent.get(h).map(|s| (h.clone(), s.clone()))).collect()
        }
    };
    let (cast_signers, active) = join(cast_signers, signer_apps(inv, fid)).await;
    if cast_signers.is_empty() {
        return Vec::new();
    }
    attribute(inv, fid, &cast_signers, active).await
}

/// Fill in origins for `targets`, resolving at most `max_authors` authors
/// upstream this time. Returns how many targets are still unresolved.
pub async fn origins(inv: &Invocation, notes: &mut Annotations, targets: &[Target], max_authors: usize) -> u32 {
    let wanted: std::collections::HashSet<&str> = targets.iter().map(|(_, h)| h.as_str()).collect();
    notes.origins.retain(|hash, _| wanted.contains(hash.as_str()));
    ORIGINS.with(|o| {
        let o = o.borrow();
        for (_, hash) in targets {
            if !notes.origins.contains_key(hash)
                && let Some(origin) = o.get(hash)
            {
                notes.origins.insert(hash.clone(), origin.clone());
            }
        }
    });

    // Group what is left by author, keeping display order.
    let mut groups: Vec<(u64, Vec<String>)> = Vec::new();
    for (fid, hash) in targets {
        if notes.origins.contains_key(hash) || *fid == 0 {
            continue;
        }
        match groups.iter_mut().find(|(f, _)| f == fid) {
            Some((_, hashes)) => hashes.push(hash.clone()),
            None => groups.push((*fid, vec![hash.clone()])),
        }
    }
    // Two requests per author (plus a rare fallback); keep a reserve for the
    // apps lookup and the cache write.
    let affordable = (inv.remaining_subrequests().saturating_sub(4) / 3) as usize;
    let batch: Vec<(u64, Vec<String>)> = groups.into_iter().take(max_authors.min(affordable)).collect();
    let resolved = join_all(batch.into_iter().map(|(fid, hashes)| resolve_author(inv, fid, hashes))).await;
    for (hash, origin) in resolved.into_iter().flatten() {
        notes.origins.insert(hash, origin);
    }
    targets.iter().filter(|(_, h)| !notes.origins.contains_key(h)).count() as u32
}

/// Fill in the app accounts referenced by `notes.origins`.
pub async fn apps(inv: &Invocation, notes: &mut Annotations) {
    let now = inv.now();
    let wanted: Vec<u64> = {
        let mut fids: Vec<u64> = notes.origins.values().filter_map(|o| o.app_fid).collect();
        fids.sort_unstable();
        fids.dedup();
        fids
    };
    notes.apps.retain(|fid, _| wanted.contains(fid));
    let mut missing = Vec::new();
    APPS.with(|a| {
        let a = a.borrow();
        for fid in &wanted {
            if notes.apps.contains_key(fid) {
                continue;
            }
            match a.get(fid) {
                Some((at, app)) if now - at < APPS_TTL_MS => {
                    notes.apps.insert(*fid, app.clone());
                }
                _ => missing.push(*fid),
            }
        }
    });
    if missing.is_empty() || inv.remaining_subrequests() < 2 {
        return;
    }
    if let Ok(users) = hypersnap::users_bulk(inv, &missing).await {
        APPS.with(|a| {
            let mut a = a.borrow_mut();
            for raw in &users {
                let app = normalize::user_summary(raw);
                a.insert(app.fid, (now, app.clone()));
                notes.apps.insert(app.fid, app);
            }
        });
    }
}

/// Scores, origins and apps for a page, with scores and origins fetched
/// concurrently.
pub async fn annotate(inv: &Invocation, notes: &mut Annotations, fids: &[u64], targets: &[Target], max_authors: usize) {
    let mut origin_notes = std::mem::take(notes);
    let (scores, pending) = join(scores(inv, fids), origins(inv, &mut origin_notes, targets, max_authors)).await;
    *notes = origin_notes;
    notes.merge_scores(scores, fids);
    notes.pending = pending;
    apps(inv, notes).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FetchError, Host, HttpRequest, HttpResponse};
    use futures::future::{FutureExt, LocalBoxFuture};

    /// A host that must not be reached: the invocation has no budget left.
    struct NoNetwork;

    impl Host for NoNetwork {
        fn now(&self) -> f64 {
            1000.0
        }

        fn fetch(&self, _: HttpRequest) -> LocalBoxFuture<'static, Result<HttpResponse, FetchError>> {
            panic!("an exhausted invocation must not send requests")
        }

        fn spawn(&self, _: LocalBoxFuture<'static, ()>) {
            panic!("no background tasks expected")
        }
    }

    #[test]
    fn casts_whose_signer_history_could_not_be_read_stay_retryable() {
        let inv = Invocation::new(Rc::new(NoNetwork), 0);
        let targets = vec![("0xretry".to_owned(), "unknown-key".to_owned())];
        let first = attribute(&inv, 1, &targets, None).now_or_never().unwrap();
        assert!(first.is_empty());
        ORIGINS.with(|o| assert!(o.borrow().get(&"0xretry".to_owned()).is_none()));

        // Next time the key is known, and the cast is attributed.
        let known = Rc::new(HashMap::from([("unknown-key".to_owned(), Some(77))]));
        let retry = attribute(&inv, 1, &targets, Some(known)).now_or_never().unwrap();
        assert_eq!(retry[0].1.app_fid, Some(77));
        ORIGINS.with(|o| assert_eq!(o.borrow().get(&"0xretry".to_owned()).unwrap().app_fid, Some(77)));
    }

    #[test]
    fn a_key_known_to_have_no_app_is_recorded() {
        let inv = Invocation::new(Rc::new(NoNetwork), 0);
        let known = Rc::new(HashMap::from([("bare-key".to_owned(), None)]));
        let targets =
            vec![("0xbare".to_owned(), "bare-key".to_owned()), ("0xmissing".to_owned(), "missing-key".to_owned())];
        let resolved = attribute(&inv, 1, &targets, Some(known)).now_or_never().unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].0, "0xbare");
        assert_eq!(resolved[0].1.app_fid, None);
    }
}

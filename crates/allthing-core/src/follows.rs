// SPDX-License-Identifier: AGPL-3.0-or-later
//! Followers and following, the newest follow first, as the node lists
//! them, a page at a time.
//!
//! A follow is a message of the account that makes it, and the node keeps
//! each account in one of its shards (two on 21 October 2026). Whom an
//! account follows is all its own messages, in one shard, so the node's
//! pages of it are in order already. Its followers are in every shard: a
//! page of them (`linksByTargetFid`) holds each shard's next links, newest
//! first, one shard after the other, and one token continues every shard
//! where it stopped. The shards don't keep in step, so the next page can
//! bring links newer than the oldest of this one.
//!
//! [`merge`] lists, of the links read, only those that no later page can
//! come before: those as new as the oldest link each shard still being read
//! has reached. The others wait in the cursor ([`Paging`]) beside each
//! shard's place, and are listed with a later page's. A shard that gets a
//! page ahead of the others sits out the next read ([`Paging::ask`]), so
//! what waits stays small.
//!
//! Which shard a link came from is told by the node's token: a shard whose
//! next key came back read a full page, one that has none read to its end
//! (fewer), and one left out read nothing. The links are then counted off
//! in shard order. When the counts don't add up, that page is put in order
//! by itself, with what waited, and the list goes on.

use crate::provenance::{base64_decode, base64_encode};

/// Farcaster time counts seconds from 1 January 2021 (UTC).
pub const FARCASTER_EPOCH: i64 = 1_609_459_200;

/// A page lists at most this many people: one `user/bulk` read hydrates
/// them (it takes 150), with the list's own account on a first page.
pub const MAX_LISTED: usize = 140;

/// Links a cursor carries at most. A shard a page ahead of the others sits
/// out reads, so about a page of links waits at most (79 of 50-link pages,
/// paging @dwr's followers 25 pages deep on 21 October 2026); past this,
/// the page is put in order by itself instead.
pub const MAX_WAITING: usize = 1000;

/// A cursor is at most this many characters: [`MAX_WAITING`] links, a few
/// bytes each, and each shard's key.
pub const MAX_CURSOR_CHARS: usize = 12_000;

/// At most this many shards; a cursor naming more is refused.
const MAX_SHARDS: usize = 8;
/// A shard's key is at most this many bytes (the node's are 30 and 33).
const MAX_KEY: usize = 64;
const VERSION: u8 = 1;

/// Unix seconds of a Farcaster timestamp.
pub fn unix(ts: u32) -> i64 {
    FARCASTER_EPOCH + i64::from(ts)
}

/// One follow: the other account, and when it was made (Farcaster seconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub fid: u64,
    pub ts: u32,
}

/// Where a shard's reading goes on: the node's key, and the time of the
/// oldest link read from it, which no later link of it is newer than.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub key: Vec<u8>,
    pub last: u32,
}

/// Where a list goes on: each shard's place (`None` once it is read to its
/// end), and the links read but not listed yet, by shard, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paging {
    pub places: Vec<Option<Place>>,
    pub waiting: Vec<Vec<Link>>,
}

/// What the node answered: its links, in its order, and each shard's next
/// key (`None` at that shard's end, and for a shard left out).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Read {
    pub links: Vec<Link>,
    pub next: Vec<Option<Vec<u8>>>,
}

/// What [`merge`] makes of a read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    /// The links to list now, newest first.
    pub listed: Vec<Link>,
    /// Where the list goes on; `None` at its end.
    pub paging: Option<Paging>,
    /// Whether the order holds across pages: `false` when the read couldn't
    /// be told apart by shard, and the page was put in order by itself.
    pub exact: bool,
}

impl Paging {
    /// What to ask the node for, by shard: the key to read on from, or
    /// `None` to leave the shard out: read to its end, or a page ahead of
    /// the others (`page_size` of its links wait already). At least one
    /// shard still being read is always asked for.
    pub fn ask(&self, page_size: usize) -> Vec<Option<&[u8]>> {
        let ahead = |i: usize| self.waiting.get(i).is_some_and(|w| w.len() >= page_size);
        let behind = self.places.iter().enumerate().any(|(i, place)| place.is_some() && !ahead(i));
        self.places
            .iter()
            .enumerate()
            .map(|(i, place)| place.as_ref().filter(|_| !behind || !ahead(i)).map(|p| p.key.as_slice()))
            .collect()
    }

    /// Whether nothing is left to read or list.
    pub fn is_done(&self) -> bool {
        self.places.iter().all(Option::is_none) && self.waiting.iter().all(Vec::is_empty)
    }

    /// The cursor: URL-safe base64, without padding.
    pub fn encode(&self) -> String {
        let mut out = vec![VERSION, self.places.len() as u8];
        for (i, place) in self.places.iter().enumerate() {
            match place {
                None => out.push(0),
                Some(place) => {
                    out.push(1);
                    varint(&mut out, u64::from(place.last));
                    out.push(place.key.len() as u8);
                    out.extend_from_slice(&place.key);
                }
            }
            let waiting = self.waiting.get(i).map(Vec::as_slice).unwrap_or_default();
            varint(&mut out, waiting.len() as u64);
            let mut previous: Option<u32> = None;
            for link in waiting {
                varint(&mut out, link.fid);
                // Newest first: each time as the step back from the one before.
                varint(&mut out, u64::from(previous.map_or(link.ts, |p| p.saturating_sub(link.ts))));
                previous = Some(link.ts);
            }
        }
        base64_encode(&out).trim_end_matches('=').replace('+', "-").replace('/', "_")
    }

    /// A cursor [`Paging::encode`] made, or `None` for anything else.
    pub fn decode(cursor: &str) -> Option<Paging> {
        let url_safe = cursor.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if cursor.is_empty() || cursor.len() > MAX_CURSOR_CHARS || !url_safe {
            return None;
        }
        let bytes = base64_decode(cursor)?;
        let mut reader = Reader { bytes: &bytes, at: 0 };
        if reader.byte()? != VERSION {
            return None;
        }
        let shards = usize::from(reader.byte()?);
        if shards == 0 || shards > MAX_SHARDS {
            return None;
        }
        let (mut places, mut waiting, mut total) = (Vec::with_capacity(shards), Vec::with_capacity(shards), 0);
        for _ in 0..shards {
            let place = match reader.byte()? {
                0 => None,
                1 => {
                    let last = u32::try_from(reader.varint()?).ok()?;
                    let len = usize::from(reader.byte()?);
                    if len == 0 || len > MAX_KEY {
                        return None;
                    }
                    Some(Place { key: reader.take(len)?.to_vec(), last })
                }
                _ => return None,
            };
            let count = usize::try_from(reader.varint()?).ok()?;
            total += count;
            if total > MAX_WAITING {
                return None;
            }
            let mut links = Vec::with_capacity(count);
            let mut previous: Option<u32> = None;
            for _ in 0..count {
                let fid = reader.varint()?;
                let step = u32::try_from(reader.varint()?).ok()?;
                let ts = match previous {
                    None => step,
                    Some(p) => p.checked_sub(step)?,
                };
                links.push(Link { fid, ts });
                previous = Some(ts);
            }
            places.push(place);
            waiting.push(links);
        }
        (reader.at == bytes.len()).then_some(Paging { places, waiting })
    }
}

/// Put a read in order with what waited from earlier pages: list the links
/// no later page can come before, newest first, at most [`MAX_LISTED`], and
/// keep the rest waiting. `before` is where the list went on from (`None`
/// for its first page, where every shard was read from its start), and
/// `page_size` how many links a shard's read brings at most.
pub fn merge(before: Option<&Paging>, read: Read, page_size: usize) -> Merged {
    let shards = before.map_or(read.next.len(), |p| p.places.len()).max(1);
    let asked: Vec<bool> = match before {
        None => vec![true; shards],
        Some(paging) => paging.ask(page_size).iter().map(Option::is_some).collect(),
    };
    let next = |i: usize| read.next.get(i).cloned().flatten();
    let counted = read.next.len() == shards || (before.is_none() && read.next.is_empty());
    let Some(ranges) = counted.then(|| split(&read.links, &asked, &read.next, page_size)).flatten() else {
        return by_itself(before, &read, shards, &asked);
    };
    // A shard still being read must have read a page, newest first: its
    // oldest link is how far it has come.
    for (i, range) in ranges.iter().enumerate() {
        let links = &read.links[range.clone()];
        if asked[i] && next(i).is_some() && (links.is_empty() || links.windows(2).any(|w| w[1].ts > w[0].ts)) {
            return by_itself(before, &read, shards, &asked);
        }
    }
    let places: Vec<Option<Place>> = (0..shards)
        .map(|i| match asked[i] {
            true => next(i).map(|key| Place { key, last: read.links[ranges[i].end - 1].ts }),
            false => before.and_then(|p| p.places.get(i).cloned().flatten()),
        })
        .collect();
    let mut pool = waited(before, shards);
    for (i, range) in ranges.iter().enumerate() {
        pool.extend(read.links[range.clone()].iter().map(|link| (i, *link)));
    }
    sort(&mut pool);
    // Nothing still to read is newer than the oldest link the furthest
    // behind shard has reached.
    let frontier = places.iter().flatten().map(|p| p.last).max().unwrap_or(0);
    let listable = pool.partition_point(|(_, link)| link.ts >= frontier);
    let listed = listable.min(MAX_LISTED);
    if pool.len() - listed > MAX_WAITING {
        return by_itself(before, &read, shards, &asked);
    }
    finish(pool, listed, places, shards, true)
}

/// Each shard's links in `links`, in shard order: a shard asked for whose
/// next key came back read a full page, one without read fewer, and one not
/// asked for read none. `None` when the counts don't add up, or a shard
/// still being read sits between two of unknown length.
fn split(
    links: &[Link],
    asked: &[bool],
    next: &[Option<Vec<u8>>],
    page_size: usize,
) -> Option<Vec<std::ops::Range<usize>>> {
    let sizes: Vec<Option<usize>> = asked
        .iter()
        .enumerate()
        .map(|(i, &asked)| match (asked, next.get(i).is_some_and(Option::is_some)) {
            (false, _) => Some(0),
            (true, true) => Some(page_size),
            (true, false) => None,
        })
        .collect();
    let known: usize = sizes.iter().flatten().sum();
    let unknown: Vec<usize> = (0..sizes.len()).filter(|&i| sizes[i].is_none()).collect();
    let rest = links.len().checked_sub(known)?;
    if rest > unknown.len() * page_size {
        return None;
    }
    let mut ranges = vec![std::ops::Range::default(); sizes.len()];
    match unknown.as_slice() {
        [] if rest > 0 => return None,
        [] => {}
        [first, ..] => {
            let (first, last) = (*first, *unknown.last().unwrap_or(first));
            // A shard still being read between two that ended: its links
            // can't be counted off.
            if sizes[first..=last].iter().any(|s| s.is_some_and(|n| n > 0)) {
                return None;
            }
            // Those that ended share what is left; which of them each link
            // came from doesn't matter, as none is read again.
            let mut at = 0;
            for (i, size) in sizes.iter().enumerate() {
                let n = size.unwrap_or(if i == first { rest } else { 0 });
                ranges[i] = at..at + n;
                at += n;
            }
            return Some(ranges);
        }
    }
    let mut at = 0;
    for (i, size) in sizes.iter().enumerate() {
        let n = size.unwrap_or(0);
        ranges[i] = at..at + n;
        at += n;
    }
    Some(ranges)
}

/// A read that couldn't be told apart by shard: put it in order with what
/// waited, list it, and read on from each shard's next key as if it were
/// in step with the others, so the order holds again from the next page on.
fn by_itself(before: Option<&Paging>, read: &Read, shards: usize, asked: &[bool]) -> Merged {
    let places: Vec<Option<Place>> = (0..shards)
        .map(|i| match asked.get(i).copied().unwrap_or(true) {
            true => read.next.get(i).cloned().flatten().map(|key| Place { key, last: 0 }),
            false => before.and_then(|p| p.places.get(i).cloned().flatten()),
        })
        .collect();
    let mut pool = waited(before, shards);
    pool.extend(read.links.iter().map(|link| (0, *link)));
    sort(&mut pool);
    let listed = pool.len().min(MAX_LISTED);
    finish(pool, listed, places, shards, false)
}

/// The links that waited, with their shard.
fn waited(before: Option<&Paging>, shards: usize) -> Vec<(usize, Link)> {
    let Some(before) = before else { return Vec::new() };
    before
        .waiting
        .iter()
        .take(shards)
        .enumerate()
        .flat_map(|(i, links)| links.iter().map(move |link| (i, *link)))
        .collect()
}

/// Newest first; links of the same second by fid, so a page reads the same
/// whatever order the node gave them in.
fn sort(pool: &mut [(usize, Link)]) {
    pool.sort_by(|a, b| b.1.ts.cmp(&a.1.ts).then(a.1.fid.cmp(&b.1.fid)));
}

/// List the first `listed` of `pool`, and keep the rest waiting by shard.
fn finish(pool: Vec<(usize, Link)>, listed: usize, places: Vec<Option<Place>>, shards: usize, exact: bool) -> Merged {
    let mut waiting = vec![Vec::new(); shards];
    for (shard, link) in &pool[listed..] {
        waiting[(*shard).min(shards - 1)].push(*link);
    }
    let listed = pool[..listed].iter().map(|(_, link)| *link).collect();
    let paging = Paging { places, waiting };
    Merged { listed, paging: (!paging.is_done()).then_some(paging), exact }
}

fn varint(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(byte)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let bytes = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(bytes)
    }

    fn varint(&mut self) -> Option<u64> {
        let mut n = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            n |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(n);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in node: each shard's links, newest first, read `page_size`
    /// at a time from a position its key names, as the node pages them: a
    /// shard that filled the page gives a next key, one that didn't gives
    /// none, and one left out gives nothing.
    struct Node {
        shards: Vec<Vec<Link>>,
        page_size: usize,
        reads: usize,
    }

    impl Node {
        fn read(&mut self, ask: Option<Vec<Option<Vec<u8>>>>) -> Read {
            self.reads += 1;
            let ask = ask.unwrap_or_else(|| vec![Some(vec![0]); self.shards.len()]);
            let mut read = Read::default();
            for (shard, key) in self.shards.iter().zip(ask) {
                let Some(key) = key else {
                    read.next.push(None);
                    continue;
                };
                let from = usize::from(key[0]) * 256 + usize::from(*key.get(1).unwrap_or(&0));
                let page: Vec<Link> = shard.iter().skip(from).take(self.page_size).copied().collect();
                let to = from + page.len();
                read.next.push((page.len() == self.page_size).then(|| vec![(to / 256) as u8, (to % 256) as u8]));
                read.links.extend(page);
            }
            read
        }
    }

    /// Every page of a list, through cursors, as the Worker reads them.
    fn page_through(node: &mut Node) -> (Vec<Link>, bool, usize) {
        let (mut all, mut exact, mut cursor, mut pages) = (Vec::new(), true, None::<String>, 0);
        loop {
            let before = cursor.as_deref().map(|c| Paging::decode(c).expect("a cursor merge made"));
            let ask = before.as_ref().map(|p| p.ask(node.page_size).iter().map(|k| k.map(<[u8]>::to_vec)).collect());
            let read = node.read(ask);
            let merged = merge(before.as_ref(), read, node.page_size);
            exact &= merged.exact;
            all.extend(merged.listed);
            pages += 1;
            assert!(pages < 1000, "the list never ends");
            match merged.paging {
                Some(paging) => cursor = Some(paging.encode()),
                None => return (all, exact, pages),
            }
        }
    }

    fn sorted(shards: &[Vec<Link>]) -> Vec<Link> {
        let mut all: Vec<Link> = shards.concat();
        all.sort_by(|a, b| b.ts.cmp(&a.ts).then(a.fid.cmp(&b.fid)));
        all
    }

    /// `all` lists every link of `shards` once, newest first (follows made
    /// in the same second, in any order).
    fn assert_listed(all: &[Link], shards: &[Vec<Link>], what: &str) {
        assert!(all.windows(2).all(|w| w[0].ts >= w[1].ts), "{what}: out of time order");
        let mut got = all.to_vec();
        got.sort_by(|a, b| b.ts.cmp(&a.ts).then(a.fid.cmp(&b.fid)));
        assert_eq!(got, sorted(shards), "{what}: not every link once");
    }

    /// A shard's links, newest first, from `(fid, ts)` pairs.
    fn shard(pairs: &[(u64, u32)]) -> Vec<Link> {
        let mut links: Vec<Link> = pairs.iter().map(|&(fid, ts)| Link { fid, ts }).collect();
        links.sort_by_key(|a| std::cmp::Reverse(a.ts));
        links
    }

    /// A small deterministic generator, for lists of many links.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
    }

    #[test]
    fn farcaster_time_starts_in_2021() {
        assert_eq!(unix(0), 1_609_459_200);
        // @dwr's newest follower on 21 October 2026.
        assert_eq!(unix(183_045_115), 1_792_504_315);
    }

    #[test]
    fn two_shards_list_in_time_order_and_end() {
        // As @10000's followers paged on 21 October 2026, 20 at a time: one
        // shard ends on the second page, the other on the third.
        let mut rng = Lcg(7);
        let a: Vec<(u64, u32)> = (0..26).map(|i| (1000 + i, 100_000 + rng.next() as u32 % 50_000)).collect();
        let b: Vec<(u64, u32)> = (0..48).map(|i| (2000 + i, 100_000 + rng.next() as u32 % 50_000)).collect();
        let shards = vec![shard(&a), shard(&b)];
        let mut node = Node { shards: shards.clone(), page_size: 20, reads: 0 };
        let (all, exact, pages) = page_through(&mut node);
        assert!(exact);
        assert_listed(&all, &shards, "two shards");
        assert!(pages <= 5, "{pages} pages");
    }

    #[test]
    fn a_shard_far_ahead_sits_out_until_the_others_catch_up() {
        // One shard's followers all came last week, the other's over years:
        // its pages run far back in time while the first's stay recent.
        let recent: Vec<(u64, u32)> = (0..300).map(|i| (10_000 + i, 1_000_000 - i as u32 * 10)).collect();
        let old: Vec<(u64, u32)> = (0..300).map(|i| (20_000 + i, 1_000_000 - i as u32 * 3000)).collect();
        let shards = vec![shard(&recent), shard(&old)];
        let mut node = Node { shards: shards.clone(), page_size: 50, reads: 0 };
        let (mut cursor, mut most) = (None::<Paging>, 0);
        let mut all = Vec::new();
        loop {
            let ask = cursor.as_ref().map(|p| p.ask(50).iter().map(|k| k.map(<[u8]>::to_vec)).collect());
            let merged = merge(cursor.as_ref(), node.read(ask), 50);
            all.extend(merged.listed);
            match merged.paging {
                Some(paging) => {
                    most = most.max(paging.waiting.iter().map(Vec::len).sum::<usize>());
                    cursor = Some(Paging::decode(&paging.encode()).unwrap());
                }
                None => break,
            }
        }
        assert_eq!(all, sorted(&shards));
        // At most about two pages wait, where reading both shards every time
        // would have kept the old shard's whole list waiting.
        assert!(most <= 100, "{most} links waited");
    }

    #[test]
    fn random_lists_page_in_exact_order() {
        let mut rng = Lcg(42);
        for case in 0..60 {
            let sizes = [rng.next() as usize % 400, rng.next() as usize % 400];
            let spread = [1 + rng.next() as u32 % 5000, 1 + rng.next() as u32 % 5000];
            let shards: Vec<Vec<Link>> = (0..2)
                .map(|s| {
                    let pairs: Vec<(u64, u32)> = (0..sizes[s])
                        .map(|i| {
                            ((s as u64 + 1) * 100_000 + i as u64, 1_000_000 - (rng.next() as u32 % spread[s]) * 30)
                        })
                        .collect();
                    shard(&pairs)
                })
                .collect();
            let page_size = [5, 20, 50][case % 3];
            let mut node = Node { shards: shards.clone(), page_size, reads: 0 };
            let (all, exact, _) = page_through(&mut node);
            assert!(exact, "case {case}");
            assert_listed(&all, &shards, &format!("case {case}: sizes {sizes:?}, page {page_size}"));
        }
    }

    #[test]
    fn shards_whose_links_dont_interleave_still_split_by_count() {
        // Every link of the second shard older than the first's: one run of
        // descending times, which only the counts can split.
        let a: Vec<(u64, u32)> = (0..40).map(|i| (1 + i, 9000 - i as u32)).collect();
        let b: Vec<(u64, u32)> = (0..40).map(|i| (100 + i, 5000 - i as u32)).collect();
        let shards = vec![shard(&a), shard(&b)];
        let mut node = Node { shards: shards.clone(), page_size: 20, reads: 0 };
        let first = node.read(None);
        let merged = merge(None, first, 20);
        // The second shard's twenty wait: the first may have newer ones still.
        assert_eq!(merged.listed.len(), 20);
        assert_eq!(merged.paging.as_ref().unwrap().waiting[1].len(), 20);
        let (all, exact, _) = page_through(&mut Node { shards: shards.clone(), page_size: 20, reads: 0 });
        assert!(exact);
        assert_eq!(all, sorted(&shards));
    }

    #[test]
    fn one_shard_passes_through_in_its_order() {
        // Whom an account follows: one shard, in order already.
        let links: Vec<(u64, u32)> = (0..250).map(|i| (5000 + i, 700_000 - i as u32 * 7)).collect();
        let shards = vec![shard(&links)];
        let mut node = Node { shards: shards.clone(), page_size: 100, reads: 0 };
        let (all, exact, pages) = page_through(&mut node);
        assert!(exact);
        assert_eq!(all, sorted(&shards));
        assert_eq!(pages, 3);
        // An empty list ends at once.
        let merged = merge(None, Read { links: Vec::new(), next: vec![None] }, 100);
        assert_eq!((merged.listed.len(), merged.paging), (0, None));
        let merged = merge(None, Read::default(), 50);
        assert_eq!((merged.listed.len(), merged.paging), (0, None));
    }

    #[test]
    fn a_page_lists_at_most_what_one_bulk_read_hydrates() {
        let links: Vec<(u64, u32)> = (0..400).map(|i| (1 + i, 50_000 - i as u32)).collect();
        let shards = vec![shard(&links)];
        let mut node = Node { shards: shards.clone(), page_size: 200, reads: 0 };
        let merged = merge(None, node.read(None), 200);
        assert_eq!(merged.listed.len(), MAX_LISTED);
        let (all, exact, _) = page_through(&mut Node { shards: shards.clone(), page_size: 200, reads: 0 });
        assert!(exact);
        assert_eq!(all, sorted(&shards));
    }

    #[test]
    fn a_read_whose_counts_dont_add_up_is_put_in_order_by_itself() {
        // Two shards said to have filled a page of 20, but 30 links came.
        let read = Read {
            links: (0..30).map(|i| Link { fid: i, ts: 1000 + (i as u32 * 37) % 100 }).collect(),
            next: vec![Some(vec![1]), Some(vec![2])],
        };
        let merged = merge(None, read, 20);
        assert!(!merged.exact);
        assert_eq!(merged.listed.len(), 30);
        assert!(merged.listed.windows(2).all(|w| w[0].ts >= w[1].ts));
        // It reads on from both shards.
        let paging = merged.paging.unwrap();
        assert_eq!(paging.ask(20), vec![Some(&[1u8][..]), Some(&[2u8][..])]);
        // Links out of time order within a shard: the same.
        let read = Read { links: vec![Link { fid: 1, ts: 5 }, Link { fid: 2, ts: 9 }], next: vec![Some(vec![1])] };
        assert!(!merge(None, read, 2).exact);
    }

    #[test]
    fn cursors_round_trip_and_refuse_anything_else() {
        let paging = Paging {
            places: vec![Some(Place { key: vec![6, 0, 0, 0, 3, 10, 229], last: 182_827_901 }), None],
            waiting: vec![
                vec![Link { fid: 3_355_551, ts: 183_045_115 }, Link { fid: 18_407, ts: 182_996_268 }],
                vec![Link { fid: 2, ts: 100 }],
            ],
        };
        let cursor = paging.encode();
        assert!(cursor.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{cursor}");
        assert_eq!(Paging::decode(&cursor), Some(paging));
        for bad in ["", "a b", "AQ==", "!!!!", "AAAA", &"A".repeat(MAX_CURSOR_CHARS + 1)] {
            assert_eq!(Paging::decode(bad), None, "{bad}");
        }
        // Trailing bytes, an unknown version, too many shards.
        let encode = |bytes: &[u8]| base64_encode(bytes).trim_end_matches('=').replace('+', "-").replace('/', "_");
        let good = base64_decode(&cursor).unwrap();
        let mut long = good.clone();
        long.push(0);
        assert_eq!(Paging::decode(&encode(&long)), None);
        let mut version = good.clone();
        version[0] = 2;
        assert_eq!(Paging::decode(&encode(&version)), None);
        assert_eq!(Paging::decode(&encode(&[1, 9])), None);
        // A waiting link newer than the one before it in a shard.
        assert_eq!(
            Paging::decode(&encode(&[1, 1, 0, 2, 5, 10, 6, 0])),
            Some(Paging { places: vec![None], waiting: vec![vec![Link { fid: 5, ts: 10 }, Link { fid: 6, ts: 10 }]] })
        );
        assert_eq!(Paging::decode(&encode(&[1, 1, 0, 2, 5, 10, 6, 11])), None);
    }

    #[test]
    fn a_cursor_stays_small() {
        // Two shards' keys as the node gives them, and a page of links
        // waiting: well under a kilobyte.
        let key = vec![7u8; 33];
        let waiting: Vec<Link> =
            (0..50).map(|i| Link { fid: 3_000_000 + i * 977, ts: 183_000_000 - i as u32 * 600 }).collect();
        let paging = Paging {
            places: vec![Some(Place { key: key.clone(), last: 182_000_000 }), Some(Place { key, last: 182_500_000 })],
            waiting: vec![waiting, Vec::new()],
        };
        let cursor = paging.encode();
        assert!(cursor.len() < 700, "{} characters", cursor.len());
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! Seen casts: the casts of the front page a reader has had on screen, kept
//! by their browser (no server state, signed in or out). A later visit marks
//! them on the front page and, in hot, lists them after the casts not seen
//! yet. Nothing is left out for having been seen.
//!
//! The browser keeps the newest [`CAP`] hashes as one string, oldest first,
//! separated by spaces (`allthing.seen` in localStorage; web/paint.js reads
//! it too).

use std::collections::{HashSet, VecDeque};

/// How many hashes a browser keeps: the oldest go first.
pub const CAP: usize = 5_000;

/// A bounded set of cast hashes, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeenSet {
    order: VecDeque<String>,
    set: HashSet<String>,
    cap: usize,
}

impl SeenSet {
    pub fn new(cap: usize) -> Self {
        Self { order: VecDeque::new(), set: HashSet::new(), cap }
    }

    /// Read what [`SeenSet::to_stored`] wrote, keeping the newest `cap`.
    /// Anything that isn't a cast hash is skipped.
    pub fn parse(stored: &str, cap: usize) -> Self {
        let mut seen = Self::new(cap);
        for hash in stored.split_ascii_whitespace().filter(|h| is_hash(h)) {
            seen.insert(hash);
        }
        seen
    }

    /// As the browser keeps it.
    pub fn to_stored(&self) -> String {
        let mut out = String::with_capacity(self.order.len() * 43);
        for (i, hash) in self.order.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(hash);
        }
        out
    }

    /// Record `hash` as the newest seen; whether it is new to the set. The
    /// oldest go once there are more than `cap`.
    pub fn insert(&mut self, hash: &str) -> bool {
        if self.cap == 0 {
            return false;
        }
        let fresh = self.set.insert(hash.to_owned());
        if !fresh && let Some(at) = self.order.iter().position(|h| h == hash) {
            self.order.remove(at);
        }
        self.order.push_back(hash.to_owned());
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        fresh
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.set.contains(hash)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// Whether `s` looks like a cast hash: `0x` and hex.
fn is_hash(s: &str) -> bool {
    s.len() > 2 && s.len() <= 66 && s.starts_with("0x") && s[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// `items` with those not seen first, each part in the order it had.
pub fn unseen_first<T>(items: Vec<T>, seen: impl Fn(&T) -> bool) -> Vec<T> {
    let (old, mut fresh): (Vec<T>, Vec<T>) = items.into_iter().partition(|item| seen(item));
    fresh.extend(old);
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: u32) -> String {
        format!("0x{n:040x}")
    }

    #[test]
    fn keeps_the_newest_cap() {
        let mut seen = SeenSet::new(3);
        for n in 0..5 {
            assert!(seen.insert(&h(n)));
        }
        assert_eq!(seen.len(), 3);
        assert!(!seen.contains(&h(0)) && !seen.contains(&h(1)));
        assert!(seen.contains(&h(2)) && seen.contains(&h(4)));
    }

    #[test]
    fn seeing_again_makes_it_newest() {
        let mut seen = SeenSet::new(2);
        seen.insert(&h(1));
        seen.insert(&h(2));
        assert!(!seen.insert(&h(1)));
        seen.insert(&h(3));
        assert!(seen.contains(&h(1)) && seen.contains(&h(3)) && !seen.contains(&h(2)));
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn round_trips_and_skips_junk() {
        let mut seen = SeenSet::new(CAP);
        seen.insert(&h(1));
        seen.insert(&h(2));
        let stored = seen.to_stored();
        assert_eq!(stored, format!("{} {}", h(1), h(2)));
        assert_eq!(SeenSet::parse(&stored, CAP), seen);
        let messy = format!("  {}  nope 0x 0xzz\n{} ", h(1), h(2));
        assert_eq!(SeenSet::parse(&messy, CAP), seen);
        assert!(SeenSet::parse("", CAP).is_empty());
    }

    #[test]
    fn parse_keeps_the_newest_when_over_cap() {
        let stored = (0..10).map(h).collect::<Vec<_>>().join(" ");
        let seen = SeenSet::parse(&stored, 4);
        assert_eq!(seen.to_stored(), (6..10).map(h).collect::<Vec<_>>().join(" "));
    }

    #[test]
    fn zero_cap_keeps_nothing() {
        let mut seen = SeenSet::new(0);
        assert!(!seen.insert(&h(1)));
        assert!(seen.is_empty());
    }

    #[test]
    fn unseen_first_is_stable() {
        let items = vec![1, 2, 3, 4, 5, 6];
        assert_eq!(unseen_first(items, |n| n % 2 == 0), vec![1, 3, 5, 2, 4, 6]);
        assert_eq!(unseen_first(vec![1, 2], |_| false), vec![1, 2]);
        assert_eq!(unseen_first(vec![1, 2], |_| true), vec![1, 2]);
        assert!(unseen_first(Vec::<u8>::new(), |_| true).is_empty());
    }
}

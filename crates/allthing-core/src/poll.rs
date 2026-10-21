// SPDX-License-Identifier: AGPL-3.0-or-later
//! Allthing's own poll on the front page: "Would you like an Allthing
//! mobile client?", Yes or No, with its results shown once the reader has
//! voted.
//!
//! One vote per network, kept by the Worker (`POST /api/v1/poll`): a vote
//! counts for the address Cloudflare saw it come from, an IPv4 address, or
//! an IPv6 address's /64 prefix ([`network`]), which one device's addresses
//! rotate in. The Worker keeps only an HMAC of the poll and the network,
//! keyed with a Worker secret, and the choice: never the address. People
//! behind one address (an office, a carrier's shared NAT) share one vote.
//! An answer for a network that has voted already never says which way it
//! voted: on a shared address it may have been someone else.
//!
//! The browser keeps its own vote and the counts it was answered with
//! ([`Kept`]), so later visits show the results with no request; the front
//! page's payload carries fresher counts ([`Counts`]).

use serde::{Deserialize, Serialize};

use crate::format::{full_number, plural};

/// Where a vote goes.
pub const POLL_PATH: &str = "/api/v1/poll";
/// The poll's id: a new poll, or this one asked anew, takes another.
pub const MOBILE_CLIENT: &str = "mobile-client-v1";
pub const QUESTION: &str = "Would you like an Allthing mobile client?";
/// What the Worker answers (503) where it can't keep votes: no secret to
/// key them with, or no D1. The poll then hides itself.
pub const NOT_SET_UP: &str = "Polls aren't set up here";
/// The largest vote a Worker reads, in bytes: `{"poll":…,"choice":…}`.
pub const MAX_BODY: usize = 256;
/// What a voter key is an HMAC for, so it can stand in for no other of the
/// Worker's MACs (its key is `SESSION_SECRET`, as sessions' are).
pub const VOTER_PURPOSE: &str = "allthing poll v1";
/// The browser's localStorage key for its vote ([`Kept`]).
pub const KEPT: &str = "allthing-poll";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    Yes,
    No,
}

impl Choice {
    pub const ALL: [Choice; 2] = [Choice::Yes, Choice::No];

    /// `"yes"` or `"no"`, as votes and D1's rows spell them.
    pub fn parse(value: &str) -> Option<Choice> {
        match value {
            "yes" => Some(Choice::Yes),
            "no" => Some(Choice::No),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Choice::Yes => "yes",
            Choice::No => "no",
        }
    }

    /// What its button says.
    pub fn label(self) -> &'static str {
        match self {
            Choice::Yes => "Yes",
            Choice::No => "No",
        }
    }
}

/// A vote, as the browser sends it. An unknown choice doesn't parse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vote {
    pub poll: String,
    pub choice: Choice,
}

/// How many voted each way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    #[serde(default)]
    pub yes: u64,
    #[serde(default)]
    pub no: u64,
}

impl Counts {
    pub fn total(&self) -> u64 {
        self.yes + self.no
    }

    pub fn of(&self, choice: Choice) -> u64 {
        match choice {
            Choice::Yes => self.yes,
            Choice::No => self.no,
        }
    }

    /// Yes's and No's whole percents, which sum to 100 (the
    /// largest-remainder method), or both 0 before anyone has voted.
    pub fn percents(&self) -> [u32; 2] {
        match crate::clients::percents(&[self.yes, self.no])[..] {
            [yes, no] => [yes, no],
            _ => [0, 0],
        }
    }

    /// `choice`'s whole percent.
    pub fn percent(&self, choice: Choice) -> u32 {
        self.percents()[match choice {
            Choice::Yes => 0,
            Choice::No => 1,
        }]
    }

    /// "1 vote", "1,234 votes".
    pub fn votes(&self) -> String {
        plural(self.total(), "vote", "votes")
    }

    /// The results in a sentence, for screen readers: "Yes 63% (5), No
    /// 37% (3), 8 votes. You voted Yes." With no choice, the vote was a
    /// network's that had voted already.
    pub fn summary(&self, choice: Option<Choice>) -> String {
        let each = Choice::ALL.map(|c| format!("{} {}% ({})", c.label(), self.percent(c), full_number(self.of(c))));
        let whose = match choice {
            Some(c) => format!("You voted {}.", c.label()),
            None => ALREADY.to_owned(),
        };
        format!("{}, {}, {}. {whose}", each[0], each[1], self.votes())
    }
}

/// What a network that has voted already is told, without how it voted.
pub const ALREADY: &str = "A vote from your network was already counted.";

/// The Worker's answer to a vote: counted, or a vote from this network was
/// counted already (and how many voted each way, either way).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub counted: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already: bool,
    pub counts: Counts,
}

/// What the browser keeps of its vote (localStorage, [`KEPT`]): which poll,
/// its choice (`None`: its network had voted already), the counts it saw
/// last, and when they were counted (Unix milliseconds).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub poll: String,
    #[serde(default)]
    pub choice: Option<Choice>,
    pub counts: Counts,
    pub at: f64,
}

impl Kept {
    /// These results with `counts`, if they were counted later (`at`, the
    /// front page payload's time): a reader's own vote never seems to
    /// vanish while a cached page catches up, and later counts aren't
    /// hidden behind it.
    pub fn newer(self, counts: Counts, at: f64) -> Kept {
        if at > self.at { Kept { counts, at, ..self } } else { self }
    }
}

/// What a voter key is the HMAC of, after [`VOTER_PURPOSE`]: the poll and
/// the network, so a network's votes in two polls can't be linked.
pub fn voter_value(poll: &str, network: &str) -> String {
    format!("{poll}|{network}")
}

/// The network a vote counts for, from the address it came from
/// (`CF-Connecting-IP`): an IPv4 address, in its usual form, or an IPv6
/// address's /64 prefix (`2001:db8:1:2::/64`), since one device's addresses
/// rotate within it. An IPv4 address written as IPv6 (`::ffff:a.b.c.d`) is
/// the IPv4 address. A zone (`%eth0`) and brackets are dropped. `None` for
/// anything else.
pub fn network(address: &str) -> Option<String> {
    let address = address.trim();
    let address = address.strip_prefix('[').and_then(|a| a.strip_suffix(']')).unwrap_or(address);
    if let Some(v4) = ipv4(address) {
        return Some(dotted(v4));
    }
    let groups = ipv6(address.split('%').next().unwrap_or_default())?;
    if groups[..5] == [0; 5] && groups[5] == 0xffff {
        let [a, b] = groups[6].to_be_bytes();
        let [c, d] = groups[7].to_be_bytes();
        return Some(dotted([a, b, c, d]));
    }
    Some(format!("{:x}:{:x}:{:x}:{:x}::/64", groups[0], groups[1], groups[2], groups[3]))
}

fn dotted([a, b, c, d]: [u8; 4]) -> String {
    format!("{a}.{b}.{c}.{d}")
}

/// Four decimal numbers from 0 to 255, without leading zeros.
fn ipv4(address: &str) -> Option<[u8; 4]> {
    let mut out = [0; 4];
    let mut parts = address.split('.');
    for byte in &mut out {
        let part = parts.next()?;
        let well_formed = (1..=3).contains(&part.len())
            && part.bytes().all(|b| b.is_ascii_digit())
            && (part.len() == 1 || !part.starts_with('0'));
        *byte = if well_formed { part.parse().ok()? } else { return None };
    }
    parts.next().is_none().then_some(out)
}

/// Eight groups of up to four hex digits, `::` standing for one or more
/// groups of zeros, the last two perhaps written as an IPv4 address.
fn ipv6(address: &str) -> Option<[u16; 8]> {
    // The groups of `part`, its last two perhaps an IPv4 address.
    fn groups(part: &str, last: bool) -> Option<Vec<u16>> {
        if part.is_empty() {
            return Some(Vec::new());
        }
        let pieces: Vec<&str> = part.split(':').collect();
        let mut out = Vec::with_capacity(8);
        for (i, piece) in pieces.iter().enumerate() {
            if last && i + 1 == pieces.len() && piece.contains('.') {
                let [a, b, c, d] = ipv4(piece)?;
                out.extend([u16::from_be_bytes([a, b]), u16::from_be_bytes([c, d])]);
            } else if (1..=4).contains(&piece.len()) && piece.bytes().all(|b| b.is_ascii_hexdigit()) {
                out.push(u16::from_str_radix(piece, 16).ok()?);
            } else {
                return None;
            }
        }
        Some(out)
    }
    let full = match address.split_once("::") {
        Some((head, tail)) => {
            if tail.contains("::") {
                return None;
            }
            let (head, tail) = (groups(head, false)?, groups(tail, true)?);
            if head.len() + tail.len() > 7 {
                return None;
            }
            let mut full = head;
            full.resize(8 - tail.len(), 0);
            full.extend(tail);
            full
        }
        None => groups(address, true)?,
    };
    full.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ipv4_address_is_its_own_network() {
        assert_eq!(network("203.0.113.7").as_deref(), Some("203.0.113.7"));
        assert_eq!(network(" 198.51.100.4\n").as_deref(), Some("198.51.100.4"));
        assert_eq!(network("0.0.0.0").as_deref(), Some("0.0.0.0"));
        // Two addresses are two networks.
        assert_ne!(network("203.0.113.7"), network("203.0.113.8"));
    }

    #[test]
    fn an_ipv6_address_counts_for_its_64() {
        let full = network("2001:db8:85a3:8d3:1319:8a2e:370:7348");
        assert_eq!(full.as_deref(), Some("2001:db8:85a3:8d3::/64"));
        // Another device, or the same one's next address, in that /64.
        assert_eq!(network("2001:db8:85a3:8d3::1"), full);
        assert_eq!(network("2001:DB8:85A3:08D3:ffff:ffff:ffff:ffff"), full);
        assert_eq!(network("[2001:db8:85a3:8d3::42]"), full);
        // The next /64 is another network.
        assert_eq!(network("2001:db8:85a3:8d4::1").as_deref(), Some("2001:db8:85a3:8d4::/64"));
        // Compressed forms and zones.
        assert_eq!(network("2001:db8::1").as_deref(), Some("2001:db8:0:0::/64"));
        assert_eq!(network("::1").as_deref(), Some("0:0:0:0::/64"));
        assert_eq!(network("::").as_deref(), Some("0:0:0:0::/64"));
        assert_eq!(network("fe80::1%eth0").as_deref(), Some("fe80:0:0:0::/64"));
        assert_eq!(network("fe80::a:b:c:d%25en0").as_deref(), Some("fe80:0:0:0::/64"));
        assert_eq!(network("1:2:3:4:5:6:7::").as_deref(), Some("1:2:3:4::/64"));
        assert_eq!(network("64:ff9b::192.0.2.33").as_deref(), Some("64:ff9b:0:0::/64"));
    }

    #[test]
    fn an_ipv4_address_written_as_ipv6_is_the_ipv4_address() {
        assert_eq!(network("::ffff:198.51.100.4").as_deref(), Some("198.51.100.4"));
        assert_eq!(network("::ffff:c633:6404").as_deref(), Some("198.51.100.4"));
        assert_eq!(network("0:0:0:0:0:ffff:198.51.100.4"), network("198.51.100.4"));
    }

    #[test]
    fn anything_else_is_no_network() {
        for address in [
            "",
            " ",
            "unknown",
            "localhost",
            "256.1.1.1",
            "1.2.3",
            "1.2.3.4.5",
            "01.2.3.4",
            "1.2.3.-4",
            "203.0.113.7:443",
            "1:2:3",
            "1:2:3:4:5:6:7:8:9",
            "1::2::3",
            "12345::",
            "gggg::1",
            ":1",
            "1:",
            "::ffff:1.2.3",
            "1.2.3.4::",
            "2001:db8::/64",
            "[2001:db8::1",
        ] {
            assert_eq!(network(address), None, "{address:?}");
        }
    }

    #[test]
    fn a_voter_key_is_the_hmac_of_the_poll_and_the_network() {
        assert_eq!(voter_value(MOBILE_CLIENT, "203.0.113.7"), "mobile-client-v1|203.0.113.7");
        assert_ne!(voter_value("another-poll", "203.0.113.7"), voter_value(MOBILE_CLIENT, "203.0.113.7"));
    }

    #[test]
    fn choices_are_yes_and_no() {
        assert_eq!(Choice::parse("yes"), Some(Choice::Yes));
        assert_eq!(Choice::parse("no"), Some(Choice::No));
        for other in ["", "Yes", "YES", "maybe", "yes ", "1"] {
            assert_eq!(Choice::parse(other), None, "{other:?}");
        }
        let vote: Vote = serde_json::from_str(r#"{"poll":"mobile-client-v1","choice":"yes"}"#).unwrap();
        assert_eq!(vote, Vote { poll: MOBILE_CLIENT.into(), choice: Choice::Yes });
        for bad in [r#"{"poll":"mobile-client-v1","choice":"maybe"}"#, r#"{"poll":"mobile-client-v1"}"#, "yes"] {
            assert!(serde_json::from_str::<Vote>(bad).is_err(), "{bad}");
        }
        assert_eq!(Choice::ALL.map(Choice::label), ["Yes", "No"]);
        assert_eq!(Choice::ALL.map(|c| Choice::parse(c.as_str())), [Some(Choice::Yes), Some(Choice::No)]);
    }

    #[test]
    fn percents_sum_to_a_hundred() {
        let counts = |yes, no| Counts { yes, no };
        assert_eq!(counts(0, 0).percents(), [0, 0]);
        assert_eq!(counts(1, 0).percents(), [100, 0]);
        assert_eq!(counts(0, 3).percents(), [0, 100]);
        assert_eq!(counts(1, 1).percents(), [50, 50]);
        assert_eq!(counts(2, 1).percents(), [67, 33]);
        assert_eq!(counts(1, 2).percents(), [33, 67]);
        // Equal remainders: the first (Yes) takes the point left.
        assert_eq!(counts(3, 5).percents(), [38, 62]);
        assert_eq!(counts(5, 3).percents(), [63, 37]);
        for (yes, no) in [(1, 6), (7, 11), (999, 1), (1, 999), (12_345, 67_890)] {
            assert_eq!(counts(yes, no).percents().iter().sum::<u32>(), 100, "{yes}/{no}");
        }
        assert_eq!(counts(5, 3).percent(Choice::No), 37);
    }

    #[test]
    fn results_read_in_a_sentence() {
        let counts = Counts { yes: 5, no: 3 };
        assert_eq!(counts.votes(), "8 votes");
        assert_eq!(Counts { yes: 1, no: 0 }.votes(), "1 vote");
        assert_eq!(Counts { yes: 1_000, no: 234 }.votes(), "1,234 votes");
        assert_eq!(counts.summary(Some(Choice::Yes)), "Yes 63% (5), No 37% (3), 8 votes. You voted Yes.");
        assert_eq!(
            counts.summary(None),
            "Yes 63% (5), No 37% (3), 8 votes. A vote from your network was already counted."
        );
        assert_eq!(
            Counts { yes: 1_000, no: 234 }.summary(Some(Choice::No)),
            "Yes 81% (1,000), No 19% (234), 1,234 votes. You voted No."
        );
    }

    #[test]
    fn answers_say_counted_or_already_and_never_how_it_voted() {
        let counted = Answer { counted: true, already: false, counts: Counts { yes: 1, no: 0 } };
        assert_eq!(serde_json::to_string(&counted).unwrap(), r#"{"counted":true,"counts":{"yes":1,"no":0}}"#);
        let already = Answer { counted: false, already: true, counts: Counts { yes: 1, no: 0 } };
        let json = serde_json::to_string(&already).unwrap();
        assert_eq!(json, r#"{"counted":false,"already":true,"counts":{"yes":1,"no":0}}"#);
        assert!(!json.contains("choice"));
        assert_eq!(serde_json::from_str::<Answer>(&json).unwrap(), already);
    }

    #[test]
    fn the_browser_keeps_its_vote_and_shows_the_newer_counts() {
        let kept = Kept {
            poll: MOBILE_CLIENT.into(),
            choice: Some(Choice::No),
            counts: Counts { yes: 2, no: 1 },
            at: 1_000.0,
        };
        let json = serde_json::to_string(&kept).unwrap();
        assert_eq!(json, r#"{"poll":"mobile-client-v1","choice":"no","counts":{"yes":2,"no":1},"at":1000.0}"#);
        assert_eq!(serde_json::from_str::<Kept>(&json).unwrap(), kept);
        // A network's vote counted already keeps no choice.
        let already: Kept =
            serde_json::from_str(r#"{"poll":"mobile-client-v1","counts":{"yes":3,"no":4},"at":5}"#).unwrap();
        assert_eq!(already.choice, None);
        // A later payload's counts replace the vote's own; an earlier one's don't.
        let later = Counts { yes: 9, no: 4 };
        assert_eq!(kept.clone().newer(later, 2_000.0).counts, later);
        assert_eq!(kept.clone().newer(later, 2_000.0).choice, Some(Choice::No));
        assert_eq!(kept.clone().newer(later, 500.0), kept);
    }
}

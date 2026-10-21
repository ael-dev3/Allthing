// SPDX-License-Identifier: AGPL-3.0-or-later
//! Finding people in a profile's Followers or Following list as the reader
//! types: the search, the sorts and the A–Z letters behind the list's
//! toolbar. It knows nothing of the page: it takes each person's names,
//! bio and numbers ([`Person`]), and answers with the rows that match, in
//! order ([`People::search`]), and where to mark the matches ([`marks`]).
//!
//! Text is folded once a row ([`fold`]): lowercased, with Latin letters'
//! accents taken off by a small table rather than a Unicode normalization
//! crate, which would make the app larger. A query is words, all of which
//! must match ([`Query`]); a word that starts with `@` matches the start of
//! a username only. A row ranks by where its words match ([`Class`]): its
//! username exactly, the start of its username, the start of its name, the
//! start of a word of its name, anywhere in its name or username, the start
//! of a word of its bio. Rows that rank alike keep the chosen [`Sort`].

use std::cmp::Ordering;
use std::ops::Range;

/// What the search reads of a person in a list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Person<'a> {
    pub username: &'a str,
    pub display_name: &'a str,
    pub bio: &'a str,
    pub followers: u64,
    /// When the follow was made, Unix seconds.
    pub followed_at: i64,
    /// When the account was registered, Unix seconds (0 where unknown).
    pub registered_at: i64,
}

/// How a list is ordered: by when the follows were made (newest or oldest
/// first), by name, by followers, or by the accounts' age.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Sort {
    #[default]
    Recent,
    Oldest,
    AToZ,
    MostFollowers,
    NewestAccounts,
}

impl Sort {
    pub const ALL: [Sort; 5] = [Sort::Recent, Sort::Oldest, Sort::AToZ, Sort::MostFollowers, Sort::NewestAccounts];

    /// As a page's address spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Sort::Recent => "recent",
            Sort::Oldest => "oldest",
            Sort::AToZ => "az",
            Sort::MostFollowers => "followers",
            Sort::NewestAccounts => "newest",
        }
    }

    /// The sort an address names; Recent for any other.
    pub fn parse(value: Option<&str>) -> Sort {
        Sort::ALL.into_iter().find(|sort| Some(sort.as_str()) == value).unwrap_or_default()
    }

    /// What its menu item says.
    pub fn label(self) -> &'static str {
        match self {
            Sort::Recent => "Recent",
            Sort::Oldest => "Oldest",
            Sort::AToZ => "A–Z",
            Sort::MostFollowers => "Most followers",
            Sort::NewestAccounts => "Newest accounts",
        }
    }
}

/// Where a query's word matches a row, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// The whole username.
    Username,
    /// The start of the username.
    UsernameStart,
    /// The start of the display name.
    NameStart,
    /// The start of a later word of the display name.
    NameWord,
    /// Anywhere in the display name or the username.
    Anywhere,
    /// The start of a word of the bio.
    Bio,
}

/// One word of a query, folded.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Word {
    text: String,
    /// Typed with `@`: the start of a username only.
    username_only: bool,
}

/// What the reader typed: words, all of which must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    words: Vec<Word>,
}

impl Query {
    /// The words of `input`, split on whitespace and folded. A lone `@` is
    /// no word.
    pub fn parse(input: &str) -> Query {
        let words = input
            .split_whitespace()
            .filter_map(|word| {
                let (word, username_only) = match word.strip_prefix('@') {
                    Some(rest) => (rest, true),
                    None => (word, false),
                };
                let text = fold(word);
                (!text.is_empty()).then_some(Word { text, username_only })
            })
            .collect();
        Query { words }
    }

    /// Nothing to look for: every row matches.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

/// A person's text folded once, and their numbers.
#[derive(Clone, Debug)]
struct Row {
    username: String,
    name: String,
    bio: String,
    /// What A–Z orders by: the display name (else the username) from its
    /// first letter or digit, with a name that starts with no letter last.
    az: (bool, String),
    letter: char,
    followers: u64,
    followed_at: i64,
    registered_at: i64,
}

/// A list's people, in the order they came (the newest follow first), each
/// folded once, for every search after.
#[derive(Clone, Debug, Default)]
pub struct People {
    rows: Vec<Row>,
}

impl People {
    pub fn new() -> People {
        People::default()
    }

    /// Adds a person after those already in the list.
    pub fn push(&mut self, person: &Person) {
        let username = fold(person.username);
        let name = fold(person.display_name);
        let shown = if name.trim().is_empty() { &username } else { &name };
        let key = shown.trim_start_matches(|c: char| !c.is_alphanumeric()).to_owned();
        let letter = key.chars().next().filter(char::is_ascii_lowercase).map_or('#', |c| c.to_ascii_uppercase());
        self.rows.push(Row {
            bio: fold(person.bio),
            az: (letter == '#', key),
            letter,
            username,
            name,
            followers: person.followers,
            followed_at: person.followed_at,
            registered_at: person.registered_at,
        });
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The A–Z rail's letter of the person at `row` ('A' to 'Z', or '#'):
    /// their display name's (else their username's) first letter, past any
    /// emoji or marks before it.
    pub fn letter(&self, row: usize) -> char {
        self.rows.get(row).map_or('#', |row| row.letter)
    }

    /// The rows that match `query`, in order: the best matches first, each
    /// rank in `sort`'s order. With no query, every row, in `sort`'s order.
    pub fn search(&self, query: &Query, sort: Sort) -> Vec<usize> {
        if query.is_empty() {
            let mut all: Vec<usize> = (0..self.rows.len()).collect();
            all.sort_by(|&a, &b| self.order(sort, a, b));
            return all;
        }
        let mut found: Vec<((Class, u32), usize)> =
            self.rows.iter().enumerate().filter_map(|(i, row)| rank(row, query).map(|rank| (rank, i))).collect();
        found.sort_by(|(x, a), (y, b)| x.cmp(y).then_with(|| self.order(sort, *a, *b)));
        found.into_iter().map(|(_, i)| i).collect()
    }

    /// `a` and `b` in `sort`'s order; alike, in the list's.
    fn order(&self, sort: Sort, a: usize, b: usize) -> Ordering {
        let (x, y) = (&self.rows[a], &self.rows[b]);
        let by = match sort {
            Sort::Recent => y.followed_at.cmp(&x.followed_at),
            Sort::Oldest => x.followed_at.cmp(&y.followed_at),
            Sort::AToZ => x.az.cmp(&y.az).then_with(|| x.username.cmp(&y.username)),
            Sort::MostFollowers => y.followers.cmp(&x.followers),
            Sort::NewestAccounts => y.registered_at.cmp(&x.registered_at),
        };
        // Oldest is Recent turned round, its ties too.
        let list = if sort == Sort::Oldest { b.cmp(&a) } else { a.cmp(&b) };
        by.then(list)
    }
}

/// How well `row` matches every word of `query`: its worst word's class,
/// then all its words' classes summed. `None` if a word doesn't match.
fn rank(row: &Row, query: &Query) -> Option<(Class, u32)> {
    let mut worst = Class::Username;
    let mut sum = 0;
    for word in &query.words {
        let matched = class(&row.username, &row.name, &row.bio, word)?.0;
        worst = worst.max(matched);
        sum += matched as u32;
    }
    Some((worst, sum))
}

/// Where `word` matches best, if it does: its class, the field, and the
/// match's byte range in the folded text of that field.
fn class(username: &str, name: &str, bio: &str, word: &Word) -> Option<(Class, Field, Range<usize>)> {
    let text = word.text.as_str();
    let at = |start: usize| start..start + text.len();
    if username == text {
        return Some((Class::Username, Field::Username, at(0)));
    }
    if username.starts_with(text) {
        return Some((Class::UsernameStart, Field::Username, at(0)));
    }
    if word.username_only {
        return None;
    }
    if name.starts_with(text) {
        return Some((Class::NameStart, Field::Name, at(0)));
    }
    if let Some(start) = word_start(name, text) {
        return Some((Class::NameWord, Field::Name, at(start)));
    }
    if let Some(start) = name.find(text) {
        return Some((Class::Anywhere, Field::Name, at(start)));
    }
    if let Some(start) = username.find(text) {
        return Some((Class::Anywhere, Field::Username, at(start)));
    }
    word_start(bio, text).map(|start| (Class::Bio, Field::Bio, at(start)))
}

/// Where in `text` a word starts with `prefix`: the first match at the
/// start of the text or just after a character that is no letter or digit.
fn word_start(text: &str, prefix: &str) -> Option<usize> {
    text.match_indices(prefix)
        .map(|(start, _)| start)
        .find(|&start| text[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Username,
    Name,
    Bio,
}

/// Where a query matches a person, as byte ranges of their own text, in
/// order and apart, to mark: each word where it matches best.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub username: Vec<Range<usize>>,
    pub name: Vec<Range<usize>>,
    pub bio: Vec<Range<usize>>,
}

/// Where `query`'s words match `person` best, to mark in their username,
/// display name and bio as shown. Nothing for a person who doesn't match.
pub fn marks(person: &Person, query: &Query) -> Marks {
    let (username, username_at) = fold_mapped(person.username);
    let (name, name_at) = fold_mapped(person.display_name);
    let (bio, bio_at) = fold_mapped(person.bio);
    let mut marks = Marks::default();
    let mut found = Vec::with_capacity(query.words.len());
    for word in &query.words {
        let Some((_, field, range)) = class(&username, &name, &bio, word) else { return Marks::default() };
        found.push((field, range));
    }
    for (field, range) in found {
        let (ranges, at) = match field {
            Field::Username => (&mut marks.username, &username_at),
            Field::Name => (&mut marks.name, &name_at),
            Field::Bio => (&mut marks.bio, &bio_at),
        };
        ranges.push(at[range.start]..at[range.end]);
    }
    for ranges in [&mut marks.username, &mut marks.name, &mut marks.bio] {
        merge(ranges);
    }
    marks
}

/// `ranges` in order, overlapping or touching ones joined.
fn merge(ranges: &mut Vec<Range<usize>>) {
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut joined: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        match joined.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => joined.push(range),
        }
    }
    *ranges = joined;
}

/// `text` as the search compares it: lowercase, with the accents of Latin
/// letters, and combining marks, taken off ("Éva Łódź" is "eva lodz").
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    out.extend(text.chars().filter_map(fold_char));
    out
}

/// `text` folded, and for each byte of the folded text, and one past its
/// end, where it came from in `text`: so a match in the folded text marks
/// the characters it came from.
fn fold_mapped(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut at = Vec::with_capacity(text.len() + 1);
    for (start, c) in text.char_indices() {
        if let Some(folded) = fold_char(c) {
            out.push(folded);
            at.extend(std::iter::repeat_n(start, folded.len_utf8()));
        }
    }
    at.push(text.len());
    (out, at)
}

/// A character as the search compares it, or `None` for a combining mark,
/// which the search leaves out.
fn fold_char(c: char) -> Option<char> {
    if c.is_ascii() {
        return Some(c.to_ascii_lowercase());
    }
    if ('\u{300}'..='\u{36f}').contains(&c) {
        return None;
    }
    let lower = c.to_lowercase().next().unwrap_or(c);
    Some(match lower {
        'à'..='å' | 'ā' | 'ă' | 'ą' | 'æ' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' | 'ð' => 'd',
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'œ' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' | 'ß' => 's',
        'ţ' | 'ť' | 'ŧ' | 'ț' | 'þ' => 't',
        'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person<'a>(username: &'a str, display_name: &'a str, bio: &'a str) -> Person<'a> {
        Person { username, display_name, bio, ..Person::default() }
    }

    fn list(people: &[Person]) -> People {
        let mut list = People::new();
        for person in people {
            list.push(person);
        }
        list
    }

    /// The usernames `query` finds, in order.
    fn found<'a>(people: &[Person<'a>], query: &str, sort: Sort) -> Vec<&'a str> {
        list(people).search(&Query::parse(query), sort).into_iter().map(|i| people[i].username).collect()
    }

    #[test]
    fn folding_lowercases_and_takes_latin_accents_off() {
        assert_eq!(fold("Dan ROMERO"), "dan romero");
        assert_eq!(fold("Éva Łódź"), "eva lodz");
        assert_eq!(fold("Ångström Søren Œuvre Straße"), "angstrom soren ouvre strase");
        assert_eq!(fold("Çağrı Şahin"), "cagri sahin");
        // A combining mark goes; letters of other scripts and emoji stay.
        assert_eq!(fold("Cafe\u{301}"), "cafe");
        assert_eq!(fold("Ελένη 🎩 東京"), "ελένη 🎩 東京");
        assert_eq!(fold(""), "");
    }

    #[test]
    fn a_query_is_words_and_at_words_look_at_usernames_only() {
        assert!(Query::parse("").is_empty());
        assert!(Query::parse("  @  ").is_empty());
        let query = Query::parse("  Dan  @Rom ");
        assert_eq!(
            query.words,
            [Word { text: "dan".into(), username_only: false }, Word { text: "rom".into(), username_only: true }]
        );
    }

    #[test]
    fn matches_rank_by_where_they_are() {
        let people = [
            person("bio", "Someone", "dan is my favourite"),
            person("anywhere", "Jordan", ""),
            person("word", "Big Dan", ""),
            person("name", "Danny", ""),
            person("danprefix", "", ""),
            person("dan", "Exact", ""),
            person("none", "Nobody", "nothing here"),
        ];
        assert_eq!(found(&people, "dan", Sort::Recent), ["dan", "danprefix", "name", "word", "anywhere", "bio"]);
        // The bio counts at the start of a word only: "nothing" isn't "thing".
        assert_eq!(found(&people, "thing", Sort::Recent), Vec::<&str>::new());
        // "here" is in the middle of a username, and starts a word of a bio.
        assert_eq!(found(&people, "here", Sort::Recent), ["anywhere", "none"]);
        // A username's middle counts as anywhere.
        assert_eq!(found(&people, "prefix", Sort::Recent), ["danprefix"]);
    }

    #[test]
    fn at_words_match_the_start_of_usernames_only() {
        let people = [person("jordan", "Dan", "dan"), person("dwr", "Dan Romero", ""), person("danb", "Someone", "")];
        assert_eq!(found(&people, "@dan", Sort::Recent), ["danb"]);
        assert_eq!(found(&people, "@dwr", Sort::Recent), ["dwr"]);
        assert_eq!(found(&people, "@d", Sort::Recent), ["dwr", "danb"]);
        assert_eq!(found(&people, "@rdan", Sort::Recent), Vec::<&str>::new());
    }

    #[test]
    fn every_word_must_match_and_the_worst_decides() {
        let people = [
            person("a", "Dan Romero", ""),
            person("b", "Danielle Romanov", "rome"),
            person("c", "Dan Smith", "romantic"),
            person("d", "Smith", ""),
        ];
        // "dan" starts each name; "rom" starts a word of a's and b's name,
        // and only c's bio.
        assert_eq!(found(&people, "dan rom", Sort::Recent), ["a", "b", "c"]);
        assert_eq!(found(&people, "rom dan", Sort::Recent), ["a", "b", "c"]);
        assert_eq!(found(&people, "smith", Sort::Recent), ["d", "c"]);
        assert_eq!(found(&people, "dan xyz", Sort::Recent), Vec::<&str>::new());
    }

    #[test]
    fn accents_and_case_dont_matter() {
        let people = [person("eva", "Éva Łódź", "Café owner in Kraków")];
        for query in ["eva", "ÉVA", "éva", "lodz", "Łódź", "krakow", "cafe", "café", "Cafe\u{301}"] {
            assert_eq!(found(&people, query, Sort::Recent), ["eva"], "{query}");
        }
    }

    /// Four people: username, display name, followers, when followed, when registered.
    fn numbered() -> [Person<'static>; 4] {
        let at = |username, display_name, followers, followed_at, registered_at| Person {
            username,
            display_name,
            followers,
            followed_at,
            registered_at,
            ..Person::default()
        };
        [
            at("b", "Bea", 10, 400, 30),
            at("a", "ann", 300, 300, 10),
            at("d", "🎩 Dan", 20, 200, 40),
            at("z", "0x Zed", 300, 100, 20),
        ]
    }

    #[test]
    fn each_sort_orders_the_whole_list() {
        let people = numbered();
        assert_eq!(found(&people, "", Sort::Recent), ["b", "a", "d", "z"]);
        assert_eq!(found(&people, "", Sort::Oldest), ["z", "d", "a", "b"]);
        // By name from its first letter or digit, emoji and case aside.
        assert_eq!(found(&people, "", Sort::AToZ), ["a", "b", "d", "z"]);
        // Ties keep the list's order.
        assert_eq!(found(&people, "", Sort::MostFollowers), ["a", "z", "d", "b"]);
        assert_eq!(found(&people, "", Sort::NewestAccounts), ["d", "b", "z", "a"]);
    }

    #[test]
    fn rows_that_rank_alike_keep_the_sort() {
        let people = numbered();
        // "e" is in the middle of Bea's name and of Zed's, and nowhere else:
        // the two rank alike, in each sort's order.
        assert_eq!(found(&people, "e", Sort::Recent), ["b", "z"]);
        assert_eq!(found(&people, "e", Sort::Oldest), ["z", "b"]);
        assert_eq!(found(&people, "e", Sort::MostFollowers), ["z", "b"]);
    }

    #[test]
    fn a_to_z_letters_skip_what_comes_before_the_first_letter() {
        let people = list(&[
            person("dwr", "🎩 Dan", ""),
            person("eva", "Éva", ""),
            person("nobody", "", ""),
            person("hex", "0xael", ""),
            person("tokyo", "東京", ""),
            person("blank", "  ", ""),
        ]);
        let letters: Vec<char> = (0..people.len()).map(|row| people.letter(row)).collect();
        assert_eq!(letters, ['D', 'E', 'N', '#', '#', 'B']);
        assert_eq!(people.letter(99), '#');
        // A–Z puts the letters first, then the rest, by name.
        let order: Vec<usize> = people.search(&Query::default(), Sort::AToZ);
        assert_eq!(order, [5, 0, 1, 2, 3, 4]);
    }

    /// A mark's bytes.
    fn bytes(start: usize, end: usize) -> Range<usize> {
        start..end
    }

    #[test]
    fn marks_are_where_each_word_matched_best_in_the_text_as_shown() {
        let ann = person("ao", "Ánn Ödegaard", "Builds at Ödeon, lives in Århus");
        // The name's start, and a word of the name: bytes of the name as shown.
        let marks = super::marks(&ann, &Query::parse("an ode"));
        assert_eq!(marks.name, [0..3, 5..9]);
        assert_eq!(&ann.display_name[0..3], "Án");
        assert_eq!(&ann.display_name[5..9], "Öde");
        assert!(marks.username.is_empty() && marks.bio.is_empty());
        // The whole username, and the start of one.
        assert_eq!(super::marks(&person("ann", "Ann", ""), &Query::parse("@ann")).username, [bytes(0, 3)]);
        assert_eq!(super::marks(&person("ann", "Ann", ""), &Query::parse("an")).username, [bytes(0, 2)]);
        // The bio.
        let marks = super::marks(&ann, &Query::parse("arhus builds"));
        assert_eq!(marks.bio, [0..6, 27..33]);
        assert_eq!(&ann.bio[27..33], "Århus");
        // Words that meet are one mark; no match is no mark.
        assert_eq!(super::marks(&ann, &Query::parse("ödeg gaard")).name, [bytes(5, 14)]);
        assert_eq!(super::marks(&ann, &Query::parse("ann zzz")), Marks::default());
    }

    #[test]
    fn sorts_are_named_in_addresses() {
        for sort in Sort::ALL {
            assert_eq!(Sort::parse(Some(sort.as_str())), sort);
        }
        assert_eq!(Sort::parse(None), Sort::Recent);
        assert_eq!(Sort::parse(Some("loudest")), Sort::Recent);
        assert_eq!(Sort::ALL.map(Sort::label), ["Recent", "Oldest", "A–Z", "Most followers", "Newest accounts"]);
    }

    #[test]
    fn five_thousand_people_are_searched_well_within_a_keystroke() {
        // A big account's list, as a search loads it: names, usernames and
        // bios of the usual lengths.
        let words =
            ["dan", "builder", "eth", "farcaster", "music", "art", "zora", "base", "dev", "coffee", "nyc", "ai"];
        let names: Vec<(String, String, String)> = (0..5_000_usize)
            .map(|i| {
                let w = |n: usize| words[(i * 7 + n * 3) % words.len()];
                (
                    format!("{}{}{i}", w(0), w(1)),
                    format!("{} {} {}", w(2).to_uppercase(), w(3), i % 97),
                    format!("{} {} {} and {} {} {} — {}", w(4), w(5), w(6), w(7), w(8), w(9), "x".repeat(i % 100)),
                )
            })
            .collect();
        let mut people = People::new();
        for (i, (username, name, bio)) in names.iter().enumerate() {
            people.push(&Person {
                username,
                display_name: name,
                bio,
                followers: (i * 37 % 10_000) as u64,
                followed_at: 2_000_000_000 - i as i64,
                registered_at: (i * 7_919 % 100_000) as i64,
            });
        }
        // The slowest of each keystroke of a few searches, each sort.
        let mut slowest = std::time::Duration::ZERO;
        for typed in ["dan", "@far", "eth builder", "zzz", "coffee nyc ai"] {
            for end in 1..=typed.len() {
                let query = Query::parse(&typed[..end]);
                for sort in Sort::ALL {
                    let started = std::time::Instant::now();
                    std::hint::black_box(people.search(&query, sort));
                    slowest = slowest.max(started.elapsed());
                }
            }
        }
        // 5 ms in a release build (the app's); the tests' unoptimized build
        // is several times slower, and CI's runners are shared.
        let bound = if cfg!(debug_assertions) { 150 } else { 15 };
        println!("the slowest keystroke of 5,000 people took {slowest:?}");
        assert!(slowest.as_millis() < bound, "the slowest keystroke took {slowest:?}");
        assert_eq!(people.search(&Query::parse("zzz"), Sort::Recent), Vec::<usize>::new());
    }
}

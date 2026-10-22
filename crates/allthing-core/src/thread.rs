// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reply-tree helpers.

use std::collections::BTreeMap;

use crate::embeds::link_key;
use crate::model::{Cast, ThreadNode};

pub fn count_descendants(node: &ThreadNode) -> usize {
    node.children.iter().map(|child| 1 + count_descendants(child)).sum()
}

pub fn count_nodes(nodes: &[ThreadNode]) -> usize {
    nodes.iter().map(|node| 1 + count_descendants(node)).sum()
}

/// How many of `nodes`, from the first, hold at least `replies` replies
/// with those under them (all of them if they hold fewer): the ones a page
/// draws at once, about a screen, before the rest.
pub fn leading_with(nodes: &[ThreadNode], replies: usize) -> usize {
    let mut held = 0;
    nodes
        .iter()
        .take_while(|node| {
            let before = held;
            held += 1 + count_descendants(node);
            before < replies
        })
        .count()
}

/// Every cast in a tree, depth first, root first.
pub fn flatten(node: &ThreadNode) -> Vec<&Cast> {
    let mut out = vec![&node.cast];
    for child in &node.children {
        out.extend(flatten(child));
    }
    out
}

/// Whether a cast links to a page known to be a mini app or frame.
fn links_to_mini_app(cast: &Cast, links: &BTreeMap<String, bool>) -> bool {
    cast.link_urls().any(|url| links.get(link_key(url)).copied().unwrap_or(false))
}

/// Mark the casts that link to a known mini app page as mini apps.
pub fn mark_mini_apps(casts: &mut [Cast], links: &BTreeMap<String, bool>) {
    for cast in casts {
        cast.mini_app |= links_to_mini_app(cast, links);
    }
}

/// [`mark_mini_apps`] for every cast in a tree.
pub fn mark_mini_apps_in_tree(node: &mut ThreadNode, links: &BTreeMap<String, bool>) {
    node.cast.mini_app |= links_to_mini_app(&node.cast, links);
    for child in &mut node.children {
        mark_mini_apps_in_tree(child, links);
    }
}

/// Listings leave hidden casts (mini apps, spam) out entirely.
pub fn without_hidden(casts: Vec<Cast>) -> Vec<Cast> {
    casts.into_iter().filter(|c| !c.hidden()).collect()
}

/// Threads keep their shape: a hidden reply (mini app, spam) with no
/// replies of its own is dropped, and one with replies stays as an emptied
/// node so the replies below it still make sense.
pub fn prune_hidden(mut node: ThreadNode) -> Option<ThreadNode> {
    node.children = node.children.into_iter().filter_map(prune_hidden).collect();
    if node.cast.hidden() {
        if node.children.is_empty() {
            return None;
        }
        node.cast.strip_hidden();
    }
    Some(node)
}

/// The ancestors a post page shows as cards: up to this many (more than the
/// chain shows in full) before it collapses.
pub const CHAIN_COLLAPSE_ABOVE: usize = 3;
/// The ancestors nearest the reply that stay shown when a chain collapses.
pub const CHAIN_SHOWN_TAIL: usize = 2;

/// Ancestors read nearest first, as the walk up the chain finds them, put
/// in reading order (root first) without the hidden ones.
pub fn chain_in_order(nearest_first: Vec<Cast>) -> Vec<Cast> {
    let mut chain = without_hidden(nearest_first);
    chain.reverse();
    chain
}

/// Which ancestors of a chain of `len` are folded behind "Show N earlier"
/// (indices into the chain, root first): the root and the last few stay
/// shown. `None` when the whole chain shows.
pub fn collapsed_gap(len: usize) -> Option<std::ops::Range<usize>> {
    (len > CHAIN_COLLAPSE_ABOVE).then(|| 1..len - CHAIN_SHOWN_TAIL)
}

/// Direct replies a replies preview shows.
pub const REPLY_PREVIEW_SIZE: usize = 10;

/// The replies preview: the most engaged direct replies, without hidden
/// ones (an emptied node that a thread keeps is hidden too). Ties keep the
/// order they came in.
pub fn reply_preview(replies: Vec<Cast>) -> Vec<Cast> {
    let mut replies = without_hidden(replies);
    replies.sort_by_key(|c| std::cmp::Reverse(crate::ranking::points(c)));
    replies.truncate(REPLY_PREVIEW_SIZE);
    replies
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Embed, UserSummary};

    fn node(hash: &str, children: Vec<ThreadNode>) -> ThreadNode {
        ThreadNode {
            cast: Cast {
                hash: hash.into(),
                author: UserSummary { fid: 1, username: "a".into(), display_name: String::new(), pfp_url: None },
                text: String::new(),
                mentions: vec![],
                ts: 0,
                parent_hash: None,
                parent_url: None,
                channel: None,
                embeds: vec![],
                likes: 0,
                recasts: 0,
                replies: 0,
                mini_app: false,
                spam: false,
            },
            children,
        }
    }

    #[test]
    fn leading_replies_hold_at_least_as_many_as_asked() {
        let nodes = vec![
            node("a", vec![node("a1", vec![]), node("a2", vec![])]),
            node("b", vec![]),
            node("c", vec![node("c1", vec![])]),
            node("d", vec![]),
        ];
        assert_eq!(leading_with(&nodes, 0), 0);
        assert_eq!(leading_with(&nodes, 1), 1);
        assert_eq!(leading_with(&nodes, 3), 1);
        assert_eq!(leading_with(&nodes, 4), 2);
        assert_eq!(leading_with(&nodes, 5), 3);
        assert_eq!(leading_with(&nodes, 7), 4);
        assert_eq!(leading_with(&nodes, 50), 4);
        assert_eq!(leading_with(&[], 6), 0);
    }

    #[test]
    fn marks_casts_linking_to_mini_app_pages() {
        let linking = |hash: &str, url: &str| {
            let mut n = node(hash, vec![]);
            n.cast.embeds = vec![Embed::Link { url: url.into(), image_url: None, title: None }];
            n
        };
        let links = BTreeMap::from([
            ("https://gig-bot-2-0.vercel.app/".to_owned(), true),
            ("https://example.com/post".to_owned(), false),
        ]);
        let mut casts = vec![
            linking("app", "https://gig-bot-2-0.vercel.app/#start").cast,
            linking("page", "https://example.com/post").cast,
            linking("unknown", "https://basefriend.tech/").cast,
        ];
        mark_mini_apps(&mut casts, &links);
        let flagged: Vec<bool> = casts.iter().map(|c| c.mini_app).collect();
        assert_eq!(flagged, [true, false, false]);

        let mut tree = node("r", vec![node("a", vec![linking("deep", "https://gig-bot-2-0.vercel.app/")])]);
        mark_mini_apps_in_tree(&mut tree, &links);
        assert!(tree.children[0].children[0].cast.mini_app);
        assert!(!tree.cast.mini_app && !tree.children[0].cast.mini_app);
    }

    #[test]
    fn prunes_mini_apps_but_keeps_threads_intact() {
        let mini = |hash: &str, children| {
            let mut n = node(hash, children);
            n.cast.mini_app = true;
            n.cast.text = "play now".into();
            n
        };
        let tree =
            node("r", vec![mini("lone", vec![]), mini("parent", vec![node("child", vec![])]), node("ok", vec![])]);
        let pruned = prune_hidden(tree).unwrap();
        let hashes: Vec<&str> = pruned.children.iter().map(|c| c.cast.hash.as_str()).collect();
        assert_eq!(hashes, ["parent", "ok"]);
        assert!(pruned.children[0].cast.text.is_empty() && pruned.children[0].cast.mini_app);
        assert_eq!(pruned.children[0].children[0].cast.hash, "child");
        assert!(prune_hidden(mini("root", vec![])).is_none());
        let listing = without_hidden(vec![mini("m", vec![]).cast, node("n", vec![]).cast]);
        assert_eq!(listing.len(), 1);
    }

    #[test]
    fn counts_and_flattens() {
        let tree = node("r", vec![node("a", vec![node("a1", vec![]), node("a2", vec![])]), node("b", vec![])]);
        assert_eq!(count_descendants(&tree), 4);
        assert_eq!(count_nodes(&tree.children), 4);
        let order: Vec<&str> = flatten(&tree).iter().map(|c| c.hash.as_str()).collect();
        assert_eq!(order, ["r", "a", "a1", "a2", "b"]);
    }

    #[test]
    fn chains_read_root_first_without_hidden_casts() {
        let mut mini = node("mini", vec![]).cast;
        mini.mini_app = true;
        let nearest_first = vec![node("parent", vec![]).cast, mini, node("root", vec![]).cast];
        let chain = chain_in_order(nearest_first);
        let order: Vec<&str> = chain.iter().map(|c| c.hash.as_str()).collect();
        assert_eq!(order, ["root", "parent"]);
    }

    #[test]
    fn folds_long_chains_around_the_root_and_the_last_few() {
        assert_eq!(collapsed_gap(0), None);
        assert_eq!(collapsed_gap(3), None);
        // A chain of four: the root, one cast behind "Show 1 earlier", then two.
        assert_eq!(collapsed_gap(4), Some(1..2));
        assert_eq!(collapsed_gap(7), Some(1..5));
        // Every cast is shown or folded behind the control: none is dropped.
        for len in 0..12 {
            let shown = match collapsed_gap(len) {
                Some(gap) => len - gap.len(),
                None => len,
            };
            assert_eq!(shown + collapsed_gap(len).map_or(0, |g| g.len()), len);
        }
    }

    #[test]
    fn previews_the_most_engaged_visible_replies() {
        let reply = |hash: &str, likes: u64| {
            let mut n = node(hash, vec![]);
            n.cast.likes = likes;
            n.cast
        };
        let mut spam = reply("spam", 99);
        spam.spam = true;
        // A hidden reply kept in a thread as an emptied node is still hidden.
        let mut emptied = reply("emptied", 50);
        emptied.mini_app = true;
        emptied.strip_hidden();
        let mut replies = vec![reply("quiet", 1), spam, reply("tied-first", 5), emptied, reply("tied-second", 5)];
        replies.extend((0..10).map(|i| reply(&format!("filler-{i}"), 0)));
        let preview = reply_preview(replies);
        assert_eq!(preview.len(), REPLY_PREVIEW_SIZE);
        let hashes: Vec<&str> = preview.iter().take(4).map(|c| c.hash.as_str()).collect();
        assert_eq!(hashes, ["tied-first", "tied-second", "quiet", "filler-0"]);
        assert!(preview.iter().all(|c| !c.hidden()));
    }
}

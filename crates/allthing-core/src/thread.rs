// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reply-tree helpers.

use crate::model::{Cast, ThreadNode};

pub fn count_descendants(node: &ThreadNode) -> usize {
    node.children.iter().map(|child| 1 + count_descendants(child)).sum()
}

pub fn count_nodes(nodes: &[ThreadNode]) -> usize {
    nodes.iter().map(|node| 1 + count_descendants(node)).sum()
}

/// Every cast in a tree, depth first, root first.
pub fn flatten(node: &ThreadNode) -> Vec<&Cast> {
    let mut out = vec![&node.cast];
    for child in &node.children {
        out.extend(flatten(child));
    }
    out
}

/// Listings leave mini app casts out entirely.
pub fn without_mini_apps(casts: Vec<Cast>) -> Vec<Cast> {
    casts.into_iter().filter(|c| !c.mini_app).collect()
}

/// Threads keep their shape: a mini app reply with no replies of its own is
/// dropped, and one with replies stays as an emptied node so the replies
/// below it still make sense.
pub fn prune_mini_apps(mut node: ThreadNode) -> Option<ThreadNode> {
    node.children = node.children.into_iter().filter_map(prune_mini_apps).collect();
    if node.cast.mini_app {
        if node.children.is_empty() {
            return None;
        }
        node.cast.strip_mini_app();
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UserSummary;

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
            },
            children,
        }
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
        let pruned = prune_mini_apps(tree).unwrap();
        let hashes: Vec<&str> = pruned.children.iter().map(|c| c.cast.hash.as_str()).collect();
        assert_eq!(hashes, ["parent", "ok"]);
        assert!(pruned.children[0].cast.text.is_empty() && pruned.children[0].cast.mini_app);
        assert_eq!(pruned.children[0].children[0].cast.hash, "child");
        assert!(prune_mini_apps(mini("root", vec![])).is_none());
        let listing = without_mini_apps(vec![mini("m", vec![]).cast, node("n", vec![]).cast]);
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
}

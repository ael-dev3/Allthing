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
            },
            children,
        }
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

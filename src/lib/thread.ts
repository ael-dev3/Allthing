import { castScore } from "./ranking";
import type { ThreadNode } from "./types";

export const COMMENT_SORTS = ["top", "new", "old"] as const;
export type CommentSort = (typeof COMMENT_SORTS)[number];

export function parseCommentSort(value: unknown): CommentSort {
  return typeof value === "string" && (COMMENT_SORTS as readonly string[]).includes(value)
    ? (value as CommentSort)
    : "top";
}

function time(node: ThreadNode): number {
  return new Date(node.cast.timestamp).getTime();
}

const COMPARATORS: Record<CommentSort, (a: ThreadNode, b: ThreadNode) => number> = {
  top: (a, b) => castScore(b.cast) - castScore(a.cast) || time(a) - time(b),
  new: (a, b) => time(b) - time(a),
  old: (a, b) => time(a) - time(b),
};

/** Sort every level of a reply tree, returning new arrays. */
export function sortThread(nodes: readonly ThreadNode[], sort: CommentSort): ThreadNode[] {
  return [...nodes]
    .sort(COMPARATORS[sort])
    .map((node) => ({ ...node, children: sortThread(node.children, sort) }));
}

export function countDescendants(node: ThreadNode): number {
  return node.children.reduce((sum, child) => sum + 1 + countDescendants(child), 0);
}

export function countNodes(nodes: readonly ThreadNode[]): number {
  return nodes.reduce((sum, node) => sum + 1 + countDescendants(node), 0);
}

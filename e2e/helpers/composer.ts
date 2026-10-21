// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, type Locator } from "@playwright/test";

/*
 * The composer's text box is editable text in which each of the pack's
 * emoji is an image whose `data-text` is its shortcode as typed
 * (crates/allthing-web/src/components/editor.rs). These read it as the
 * draft it holds, and select in it by offsets into that draft (UTF-16
 * units, an emoji counting as its shortcode), as a text box's value and
 * selection would.
 */

/** The draft the box holds. */
export const draftOf = (box: Locator) =>
  box.evaluate((root) => {
    let out = "";
    root.childNodes.forEach((node, i) => {
      if (node.nodeType === Node.TEXT_NODE) out += (node.nodeValue ?? "").replace(/ /g, " ");
      else if (node instanceof HTMLImageElement) out += node.dataset.text ?? node.alt;
      else if (node.nodeName === "BR" && i < root.childNodes.length - 1) out += "\n";
    });
    return out;
  });

/** Waits until the box holds `text`. */
export const expectDraft = (box: Locator, text: string) => expect.poll(() => draftOf(box)).toBe(text);

/** Selects from `from` to `to` in the draft, as a text box's setSelectionRange would. */
export const selectIn = (box: Locator, from: number, to: number) =>
  box.evaluate(
    (root, [from, to]) => {
      const position = (at: number): [Node, number] => {
        let count = 0;
        const nodes = Array.from(root.childNodes);
        for (let i = 0; i < nodes.length; i++) {
          const node = nodes[i];
          if (node.nodeType === Node.TEXT_NODE) {
            const length = (node.nodeValue ?? "").length;
            if (at <= count + length) return [node, at - count];
            count += length;
          } else if (node instanceof HTMLImageElement) {
            const length = (node.dataset.text ?? "").length;
            if (at <= count) return [root, i];
            count += length;
            if (at < count) return [root, i + 1];
          }
        }
        const end = root.lastChild?.nodeName === "BR" ? nodes.length - 1 : nodes.length;
        return [root, end];
      };
      (root as HTMLElement).focus();
      const range = document.createRange();
      range.setStart(...position(from));
      range.setEnd(...position(to));
      const selection = getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);
    },
    [from, to],
  );

/** The selection in the box, as offsets into the draft. */
export const selectionIn = (box: Locator) =>
  box.evaluate((root) => {
    const selection = getSelection();
    if (!selection || !selection.anchorNode || !root.contains(selection.anchorNode)) return null;
    const offset = (container: Node, at: number) => {
      const range = document.createRange();
      range.setStart(root, 0);
      range.setEnd(container, at);
      const part = range.cloneContents();
      let out = 0;
      part.childNodes.forEach((node) => {
        if (node.nodeType === Node.TEXT_NODE) out += (node.nodeValue ?? "").length;
        else if (node instanceof HTMLImageElement) out += (node.dataset.text ?? "").length;
      });
      return out;
    };
    const anchor = offset(selection.anchorNode, selection.anchorOffset);
    const focus = offset(selection.focusNode!, selection.focusOffset);
    return [Math.min(anchor, focus), Math.max(anchor, focus)];
  });

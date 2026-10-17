"use client";

import { useEffect } from "react";

const ACTIVE = "kb-active";

function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

/** Visible [data-nav] items in document order; replies inside collapsed comments are skipped. */
function navItems(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>("[data-nav]")).filter(
    (el) => el.getClientRects().length > 0,
  );
}

/** The item's own primary link, not one belonging to a nested item. */
function primaryLink(item: HTMLElement): HTMLElement | undefined {
  return Array.from(item.querySelectorAll<HTMLElement>("[data-nav-primary]")).find(
    (link) => link.closest("[data-nav]") === item,
  );
}

/**
 * RES-style keyboard navigation: j/k move between posts or comments,
 * o/Enter opens a post (or toggles a comment), x collapses a comment.
 */
export function KeyboardNav() {
  useEffect(() => {
    let current: HTMLElement | null = null;

    const select = (el: HTMLElement, scroll = true) => {
      current?.classList.remove(ACTIVE);
      current = el;
      el.classList.add(ACTIVE);
      if (scroll) {
        const anchor = el.querySelector<HTMLElement>(":scope > summary") ?? el;
        anchor.scrollIntoView({ block: "nearest" });
      }
    };

    const toggle = (el: HTMLElement) => {
      if (el instanceof HTMLDetailsElement) el.open = !el.open;
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
      if (isTyping(event.target)) return;

      const items = navItems();
      if (items.length === 0) return;
      const index = current ? items.indexOf(current) : -1;

      switch (event.key) {
        case "j":
          select(items[Math.min(index + 1, items.length - 1)]);
          break;
        case "k":
          select(items[index <= 0 ? 0 : index - 1]);
          break;
        case "o":
        case "Enter": {
          // Let Enter keep its native meaning on focused links and toggles.
          if (event.key === "Enter" && event.target !== document.body) return;
          if (!current || index === -1) return;
          const link = primaryLink(current);
          if (link) link.click();
          else toggle(current);
          break;
        }
        case "x":
          if (!current || index === -1) return;
          toggle(current);
          break;
        case "Escape":
          current?.classList.remove(ACTIVE);
          current = null;
          break;
        default:
          return;
      }
      event.preventDefault();
    };

    // Clicking inside an item makes it the current one, without scrolling.
    const onClick = (event: MouseEvent) => {
      const item = (event.target as Element | null)?.closest?.("[data-nav]");
      if (item instanceof HTMLElement) select(item, false);
    };

    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("click", onClick);
    document.documentElement.dataset.keyboardNav = "ready";
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("click", onClick);
      delete document.documentElement.dataset.keyboardNav;
    };
  }, []);

  return null;
}

/**
 * Walk up from `start` looking for the nearest Radix ScrollArea
 * viewport. Used by `ThreadedComments` to locate the sidebar's scrolling
 * container so it can snapshot/restore scrollTop across refetches
 * (rem-8w5 — posting a comment must not scroll the thread to the top).
 *
 * Lives in its own module so unit tests can import it without pulling
 * in React or obsidian, neither of which are available in the test
 * loader environment.
 */
export function findRadixScrollViewport(start: HTMLElement | null): HTMLElement | null {
  let node: HTMLElement | null = start;
  while (node) {
    if (node.hasAttribute("data-radix-scroll-area-viewport")) {
      return node;
    }
    node = node.parentElement;
  }
  return null;
}

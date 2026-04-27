/**
 * How long the `remargin-highlight` class lingers after a focus call
 * (ms). Exported so tests can advance fake timers by exactly this
 * duration to assert the class clears.
 */
export const HIGHLIGHT_DURATION_MS = 1000;

/**
 * Minimum surface a `focusCardInRoot` root needs. Real Obsidian wires a
 * full `HTMLElement`; tests pass a fake-DOM stub that satisfies just
 * these methods.
 */
export interface FocusCardRoot {
  querySelector(selector: string): FocusCardCard | null;
}

/**
 * Surface a single comment card exposes to the focus path. Real
 * `HTMLElement` satisfies it; tests pass a stub that records the
 * scroll + classList calls.
 */
export interface FocusCardCard {
  scrollIntoView(options?: ScrollIntoViewOptions | boolean): void;
  classList: { add(name: string): void; remove(name: string): void };
}

/**
 * Look up the comment card by id, scroll it into view, and apply the
 * `remargin-highlight` class for `HIGHLIGHT_DURATION_MS`. Pure and
 * UI-framework-agnostic so the unit tests can exercise the flow with a
 * lightweight fake root — the React `SidebarShell` calls this helper
 * inside its `useEffect` after a `remargin:focus` event arrives.
 *
 * Returns `true` when a card was found (and the side effects fired);
 * `false` when no card matched the id, in which case the call is a
 * silent no-op.
 */
export function focusCardInRoot(
  root: FocusCardRoot,
  commentId: string,
  setTimeoutFn: (handler: () => void, ms: number) => unknown = (h, ms) => setTimeout(h, ms)
): boolean {
  const card = root.querySelector(`[data-comment-id="${cssEscape(commentId)}"]`);
  if (!card) return false;
  card.scrollIntoView({ block: "center", behavior: "smooth" });
  card.classList.add("remargin-highlight");
  setTimeoutFn(() => {
    card.classList.remove("remargin-highlight");
  }, HIGHLIGHT_DURATION_MS);
  return true;
}

/**
 * Escape a comment id for safe use inside `[data-comment-id="..."]`.
 * Mirrors `CSS.escape` when available; falls back to escaping just the
 * characters that would terminate the attribute selector.
 */
function cssEscape(value: string): string {
  const cssGlobal = (globalThis as { CSS?: { escape?: (s: string) => string } }).CSS;
  if (cssGlobal?.escape) {
    return cssGlobal.escape(value);
  }
  return value.replace(/(["\\])/g, "\\$1");
}

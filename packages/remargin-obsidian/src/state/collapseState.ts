/**
 * Listener invoked whenever a comment's collapse state changes. Receives
 * the comment id that flipped and its new collapsed value.
 */
export type CollapseListener = (commentId: string, collapsed: boolean) => void;

/**
 * Per-session collapse state for remargin widget comments, shared between
 * the reading-mode post-processor and the Live Preview CM6 widget so
 * collapsing in one surface mirrors in the other.
 *
 * State is held in a private map keyed by comment id; default for any
 * unknown id is "collapsed" (matches the design's "land minimal, expand
 * on demand" stance). State is not persisted to plugin data — the user
 * confirmed per-session scope, and resetting on plugin reload keeps the
 * UI behaviour predictable.
 *
 * Subscribers are notified synchronously on every `toggle`; `subscribe`
 * returns an unsubscribe thunk so callers (typically React `useEffect`
 * cleanups) can detach without holding a reference to the listener.
 */
export class CollapseState {
  private readonly state = new Map<string, boolean>();
  private readonly listeners = new Set<CollapseListener>();

  /**
   * Returns whether the given comment id is currently collapsed. Defaults
   * to true for any id the store has never seen.
   */
  isCollapsed(commentId: string): boolean {
    return this.state.get(commentId) ?? true;
  }

  /**
   * Flip the collapsed flag for the given id and notify every subscriber
   * with the new value. Toggling an unknown id seeds it as "expanded"
   * (the inverse of the default-collapsed read).
   */
  toggle(commentId: string): void {
    const next = !this.isCollapsed(commentId);
    this.state.set(commentId, next);
    for (const listener of this.listeners) {
      listener(commentId, next);
    }
  }

  /**
   * Register a listener and return an unsubscribe thunk. Calling the
   * thunk multiple times is safe; the second call is a no-op since the
   * listener has already been removed.
   */
  subscribe(listener: CollapseListener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }
}

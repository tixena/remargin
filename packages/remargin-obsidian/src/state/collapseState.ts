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
   * True when the user has explicitly set a collapse value for this id.
   * Lets callers distinguish "user has not touched this" from "user
   * explicitly collapsed it" — the auto-expand priming logic uses this
   * so it doesn't override the user's deliberate collapse choice on
   * subsequent re-mounts.
   */
  has(commentId: string): boolean {
    return this.state.has(commentId);
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
   * Mark a single id as expanded (collapsed = false). No-op + no
   * notification when the id is already known to be expanded — keeps
   * the auto-expand priming idempotent across re-mounts.
   */
  setExpanded(commentId: string): void {
    if (this.state.get(commentId) === false) return;
    this.state.set(commentId, false);
    for (const listener of this.listeners) {
      listener(commentId, false);
    }
  }

  /**
   * Mark a single id as collapsed (collapsed = true). No-op + no
   * notification when the id is already known to be collapsed.
   */
  setCollapsed(commentId: string): void {
    if (this.state.get(commentId) === true) return;
    this.state.set(commentId, true);
    for (const listener of this.listeners) {
      listener(commentId, true);
    }
  }

  /**
   * Bulk-set every id in `commentIds` to `collapsed`, firing one
   * notification per id that actually changed value. Used by the
   * per-block "Expand all" / "Collapse all" toolbar so a single click
   * does not produce N React renders for the N untouched ids.
   */
  setMany(commentIds: readonly string[], collapsed: boolean): void {
    for (const id of commentIds) {
      const prev = this.state.get(id);
      if (prev === collapsed) continue;
      this.state.set(id, collapsed);
      for (const listener of this.listeners) {
        listener(id, collapsed);
      }
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

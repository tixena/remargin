/** The inbox filter modes: their labels, their `remargin query` options and empty-state copy. */

import type { QueryOpts } from "@/backend/types";
import type { InboxFilter } from "@/types";

/** One entry of the filter dropdown. */
interface InboxFilterOption {
  value: InboxFilter;
  label: string;
}

export const INBOX_FILTER_OPTIONS: readonly InboxFilterOption[] = [
  { value: "for-me", label: "Pending for me" },
  { value: "from-me", label: "Pending from me" },
  { value: "unassigned", label: "Pending unassigned" },
  { value: "pending", label: "Pending" },
  { value: "all", label: "All" },
];

/**
 * Map a filter mode onto the `remargin query` options that narrow the
 * fetch server-side. Every mode is one query — no mode filters the
 * fetched set client-side.
 *
 * Returns `null` for a mode that needs the caller's identity when it has
 * not resolved: the caller renders a notice instead of issuing a query
 * that would silently match nothing. `for-me` and `unassigned` need no
 * client-side identity because the CLI resolves the caller itself.
 */
export function inboxFilterQueryOpts(filter: InboxFilter, me: string | null): QueryOpts | null {
  switch (filter) {
    case "for-me":
      return { pendingForMe: true, expanded: true };
    case "from-me":
      return me ? { author: me, pending: true, expanded: true } : null;
    case "unassigned":
      return { pendingBroadcast: true, expanded: true };
    case "pending":
      return { pending: true, expanded: true };
    case "all":
      return { expanded: true };
  }
}

export function inboxFilterLabel(filter: InboxFilter): string {
  return INBOX_FILTER_OPTIONS.find((o) => o.value === filter)?.label ?? filter;
}

/** Copy shown in place of the list when a mode returns nothing. */
export function inboxEmptyMessage(filter: InboxFilter): string {
  switch (filter) {
    case "for-me":
      return "Nothing pending for you.";
    case "from-me":
      return "Nothing waiting on others.";
    case "unassigned":
      return "No unassigned pending comments.";
    case "pending":
      return "No pending comments.";
    case "all":
      return "No comments found.";
  }
}

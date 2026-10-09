/** The identity of one inbox fetch. */

import type { InboxFilter } from "@/types";

/**
 * Identity of one inbox fetch. Two runs sharing a token issue the same
 * query, so a response whose token is no longer the newest issued one has
 * been superseded and must not overwrite the newer result. `generation` is
 * the sidebar's refresh counter: a bump means "fetch again even though the
 * query is unchanged", which is what makes it part of the identity.
 */
export function inboxFetchToken(
  generation: number,
  filter: InboxFilter,
  me: string | null,
  search: string
): string {
  return JSON.stringify([generation, filter, me, search]);
}

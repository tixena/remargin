/** Hook exposing the vault's registered participants. */

import { useEffect, useState } from "react";
import type { Participant } from "@/backend";
import {
  loadParticipants,
  participantsCacheKey,
  resolveDisplayNameFrom,
} from "./participantsCache";
import { useBackend } from "./useBackend";
import { usePlugin } from "./usePlugin";

/** What {@link useParticipants} returns. */
export interface UseParticipantsResult {
  participants: Participant[];
  /** Falls back to the id when the registry has no entry or the fetch has not settled. */
  resolveDisplayName: (id: string) => string;
  loading: boolean;
  error: string | null;
}

// One fetch promise shared by every hook consumer in a plugin session, invalidated when the
// settings fingerprint changes.
let cachedKey: string | null = null;
let cachedPromise: Promise<Participant[]> | null = null;

/**
 * Expose the vault's registered participants and a display-name resolver
 * to React components. The underlying CLI call runs at most once per
 * plugin session for a given settings fingerprint, and re-runs whenever
 * the user edits the settings fields that affect registry resolution.
 *
 * Returns:
 *
 * - `participants` — latest result (empty until the fetch resolves, or
 *   permanently empty when the vault has no registry).
 * - `resolveDisplayName(id)` — returns the display name, or the id when
 *   no match is found or the fetch has not yet resolved.
 * - `loading` — `true` until the first fetch settles.
 * - `error` — currently always `null`.
 */
export function useParticipants(): UseParticipantsResult {
  const backend = useBackend();
  const plugin = usePlugin();
  const key = participantsCacheKey(plugin.settings);

  if (cachedKey !== key) {
    cachedKey = key;
    cachedPromise = loadParticipants(backend);
  }

  const [participants, setParticipants] = useState<Participant[]>([]);
  const [loading, setLoading] = useState<boolean>(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const currentPromise = cachedPromise;
    const currentKey = key;
    void currentPromise?.then((result) => {
      if (cancelled) return;
      // Accept the result only if the cache is still ours: settings may have flipped mid-await.
      if (cachedKey !== currentKey) return;
      setParticipants(result);
      setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [key]);

  return {
    participants,
    resolveDisplayName: (id: string) => resolveDisplayNameFrom(participants, id),
    loading,
    error: null,
  };
}

/**
 * Test-only hook cache reset. Imported by unit tests so each test runs
 * against a clean module state; not intended for production use.
 */
export function __resetParticipantsCacheForTests(): void {
  cachedKey = null;
  cachedPromise = null;
}

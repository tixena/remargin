import { useEffect, useState } from "react";
import type { Participant } from "@/backend";
import {
  loadParticipants,
  participantsCacheKey,
  resolveDisplayNameFrom,
} from "./participantsCache";
import { useBackend } from "./useBackend";
import { usePlugin } from "./usePlugin";

export interface UseParticipantsResult {
  participants: Participant[];
  /**
   * Map a participant id to the registered display name, or back to the
   * id when the registry is silent on it. Safe to call before the fetch
   * settles — it returns the id as a fallback in that case.
   */
  resolveDisplayName: (id: string) => string;
  loading: boolean;
  error: string | null;
}

// Module-level cache so all hook consumers in a plugin session share a
// single fetch promise. Invalidated when the fingerprint of relevant
// settings changes (see `participantsCacheKey`).
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
 * - `error` — `null` today; reserved for when task 33 wires up the
 *   user-facing error banner.
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

  // biome-ignore lint/correctness/useExhaustiveDependencies: `key` drives module-level cache invalidation; the effect body reads the refreshed cache promise but never references `key` directly.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const currentPromise = cachedPromise;
    const currentKey = key;
    void currentPromise?.then((result) => {
      if (cancelled) return;
      // Guard against a settings flip that happened while we were
      // awaiting — only accept the result if the cache is still ours.
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

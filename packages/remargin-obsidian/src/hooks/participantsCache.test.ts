import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { Participant, RemarginBackend } from "../backend/index.ts";
import { DEFAULT_SETTINGS, type RemarginSettings } from "../types.ts";
import {
  loadParticipants,
  participantsCacheKey,
  resolveDisplayNameFrom,
} from "./participantsCache.ts";

function settings(overrides: Partial<RemarginSettings>): RemarginSettings {
  return { ...DEFAULT_SETTINGS, ...overrides };
}

function participant(overrides: Partial<Participant>): Participant {
  return {
    name: "alice",
    display_name: "Alice Doe",
    type: "human",
    status: "active",
    pubkeys: 1,
    ...overrides,
  };
}

// Minimal RemarginBackend stand-in exposing only what `loadParticipants`
// actually calls. We use `as unknown as RemarginBackend` at the call site
// to avoid pulling in the real class (which imports node:fs / node:child).
interface RegistryStub {
  registryShow: () => Promise<Participant[]>;
}

describe("participantsCacheKey", () => {
  it("produces a stable key for identical settings", () => {
    const a = settings({ workingDirectory: "/vault" });
    const b = settings({ workingDirectory: "/vault" });
    assert.strictEqual(participantsCacheKey(a), participantsCacheKey(b));
  });

  it("changes when workingDirectory changes", () => {
    const a = settings({ workingDirectory: "/vault-a" });
    const b = settings({ workingDirectory: "/vault-b" });
    assert.notStrictEqual(participantsCacheKey(a), participantsCacheKey(b));
  });

  it("changes when remarginPath changes", () => {
    const a = settings({ remarginPath: "/usr/bin/remargin" });
    const b = settings({ remarginPath: "~/.cargo/bin/remargin" });
    assert.notStrictEqual(participantsCacheKey(a), participantsCacheKey(b));
  });

  it("ignores unrelated fields like sidebar side", () => {
    const a = settings({ sidebarSide: "left" });
    const b = settings({ sidebarSide: "right" });
    assert.strictEqual(participantsCacheKey(a), participantsCacheKey(b));
  });
});

describe("resolveDisplayNameFrom", () => {
  it("returns display_name when the id is in the registry", () => {
    const participants = [
      participant({ name: "alice", display_name: "Alice Doe" }),
      participant({ name: "bob", display_name: "Bob Smith" }),
    ];
    assert.strictEqual(resolveDisplayNameFrom(participants, "alice"), "Alice Doe");
    assert.strictEqual(resolveDisplayNameFrom(participants, "bob"), "Bob Smith");
  });

  it("falls back to the id when no display name is set (CLI fallback already applied)", () => {
    // The CLI always emits a non-empty `display_name`; this edge case
    // covers manual construction where the field is empty.
    const participants = [participant({ name: "ci", display_name: "" })];
    assert.strictEqual(resolveDisplayNameFrom(participants, "ci"), "ci");
  });

  it("falls back to the id when it is not in the registry", () => {
    const participants = [participant({ name: "alice" })];
    assert.strictEqual(resolveDisplayNameFrom(participants, "unknown"), "unknown");
  });

  it("falls back to the id when called before the fetch resolves", () => {
    assert.strictEqual(resolveDisplayNameFrom(undefined, "alice"), "alice");
  });

  it("is safe with an empty participant list (no-registry vault)", () => {
    assert.strictEqual(resolveDisplayNameFrom([], "alice"), "alice");
  });
});

describe("loadParticipants", () => {
  it("passes through a successful registryShow result", async () => {
    const fixture: Participant[] = [participant({ name: "alice" })];
    const stub: RegistryStub = {
      registryShow: () => Promise.resolve(fixture),
    };
    const result = await loadParticipants(stub as unknown as RemarginBackend);
    assert.deepStrictEqual(result, fixture);
  });

  it("returns [] when registryShow rejects", async () => {
    const stub: RegistryStub = {
      registryShow: () => Promise.reject(new Error("spawn failure")),
    };
    // Silence the expected console.error for the duration of this test.
    const originalError = console.error;
    console.error = () => {
      // intentionally empty: loadParticipants logs the error and we
      // don't want the noise in test output.
    };
    try {
      const result = await loadParticipants(stub as unknown as RemarginBackend);
      assert.deepStrictEqual(result, []);
    } finally {
      console.error = originalError;
    }
  });
});

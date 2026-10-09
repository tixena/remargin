/** Tests for the identity probe's gate value. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { identityProbeKey } from "./inboxIdentityProbe.ts";

describe("identityProbeKey", () => {
  it("probes on the first render while the identity is unresolved", () => {
    assert.equal(identityProbeKey(null, 0), 0);
  });

  it("changes with refreshKey while unresolved, so a refresh re-probes", () => {
    assert.notEqual(identityProbeKey(null, 1), identityProbeKey(null, 2));
  });

  it("pins to null once an identity resolves", () => {
    assert.equal(identityProbeKey("alice", 0), null);
  });

  it("stays null across refreshes once resolved, so no re-probe runs", () => {
    assert.equal(identityProbeKey("alice", 7), identityProbeKey("alice", 8));
  });

  it("treats a missing refreshKey as the initial counter value", () => {
    assert.equal(identityProbeKey(null, undefined), 0);
  });
});

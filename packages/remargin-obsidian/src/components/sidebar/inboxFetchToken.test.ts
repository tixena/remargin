/** Tests for the inbox fetch token. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { inboxFetchToken } from "./inboxFetchToken.ts";

describe("inboxFetchToken", () => {
  it("matches itself for an identical request", () => {
    assert.equal(
      inboxFetchToken(3, "for-me", "alice", "bug"),
      inboxFetchToken(3, "for-me", "alice", "bug")
    );
  });

  it("changes with the generation, so a sidebar mutation supersedes the in-flight fetch", () => {
    assert.notEqual(
      inboxFetchToken(1, "for-me", "alice", ""),
      inboxFetchToken(2, "for-me", "alice", "")
    );
  });

  it("changes with the filter mode", () => {
    assert.notEqual(
      inboxFetchToken(0, "for-me", "alice", ""),
      inboxFetchToken(0, "all", "alice", "")
    );
  });

  it("changes with the resolved identity, which narrows the from-me query", () => {
    assert.notEqual(
      inboxFetchToken(0, "from-me", null, ""),
      inboxFetchToken(0, "from-me", "alice", "")
    );
  });

  it("separates an unresolved identity from an author literally named null", () => {
    assert.notEqual(
      inboxFetchToken(0, "from-me", null, ""),
      inboxFetchToken(0, "from-me", "null", "")
    );
  });

  it("changes with the submitted search", () => {
    assert.notEqual(
      inboxFetchToken(0, "all", "alice", "bug"),
      inboxFetchToken(0, "all", "alice", "fix")
    );
  });

  it("keeps fields apart, so search text cannot forge another request's token", () => {
    assert.notEqual(
      inboxFetchToken(0, "all", "alice", "bob"),
      inboxFetchToken(0, "all", 'alice","bob', "")
    );
  });
});

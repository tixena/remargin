/** Tests for the `remargin query` flag mapping. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { buildQueryArgs } from "./buildQueryArgs.ts";

describe("buildQueryArgs", () => {
  it("emits just the subcommand and path with no options", () => {
    assert.deepStrictEqual(buildQueryArgs("."), ["query", "."]);
  });

  it("maps pendingForMe to the valueless --pending-for-me flag", () => {
    assert.deepStrictEqual(buildQueryArgs(".", { pendingForMe: true, expanded: true }), [
      "query",
      ".",
      "--pending-for-me",
      "--expanded",
    ]);
  });

  it("maps pendingBroadcast to the valueless --pending-broadcast flag", () => {
    assert.deepStrictEqual(buildQueryArgs(".", { pendingBroadcast: true, expanded: true }), [
      "query",
      ".",
      "--pending-broadcast",
      "--expanded",
    ]);
  });

  it("keeps --pending-for's value adjacent to its flag", () => {
    assert.deepStrictEqual(buildQueryArgs(".", { pendingFor: "alice" }), [
      "query",
      ".",
      "--pending-for",
      "alice",
    ]);
  });

  it("composes --author with --pending for the from-me mode", () => {
    assert.deepStrictEqual(
      buildQueryArgs(".", { author: "alice", pending: true, expanded: true }),
      ["query", ".", "--pending", "--author", "alice", "--expanded"]
    );
  });

  it("composes the search flags with a pending flavor in one invocation", () => {
    assert.deepStrictEqual(
      buildQueryArgs(".", {
        pendingForMe: true,
        expanded: true,
        contentRegex: "[Hh]ello",
        ignoreCase: true,
      }),
      [
        "query",
        ".",
        "--pending-for-me",
        "--expanded",
        "--content-regex",
        "[Hh]ello",
        "--ignore-case",
      ]
    );
  });

  it("omits every flag whose option is false or absent", () => {
    assert.deepStrictEqual(
      buildQueryArgs("notes", {
        pending: false,
        pendingForMe: false,
        pendingBroadcast: false,
        expanded: false,
        ignoreCase: false,
      }),
      ["query", "notes"]
    );
  });
});

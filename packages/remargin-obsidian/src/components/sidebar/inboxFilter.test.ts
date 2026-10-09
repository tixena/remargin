/** Tests for the inbox filter options, query mapping and empty-state copy. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import {
  INBOX_FILTER_OPTIONS,
  inboxEmptyMessage,
  inboxFilterLabel,
  inboxFilterQueryOpts,
} from "./inboxFilter.ts";

describe("INBOX_FILTER_OPTIONS", () => {
  it("lists the five modes in dropdown order", () => {
    assert.deepStrictEqual(
      INBOX_FILTER_OPTIONS.map((o) => o.value),
      ["for-me", "from-me", "unassigned", "pending", "all"]
    );
  });

  it("labels each mode", () => {
    assert.deepStrictEqual(
      INBOX_FILTER_OPTIONS.map((o) => o.label),
      ["Pending for me", "Pending from me", "Pending unassigned", "Pending", "All"]
    );
  });

  it("resolves a label per mode", () => {
    assert.strictEqual(inboxFilterLabel("for-me"), "Pending for me");
    assert.strictEqual(inboxFilterLabel("all"), "All");
  });
});

describe("inboxFilterQueryOpts", () => {
  it("for-me asks the CLI to resolve the caller", () => {
    assert.deepStrictEqual(inboxFilterQueryOpts("for-me", "alice"), {
      pendingForMe: true,
      expanded: true,
    });
  });

  it("for-me needs no client-side identity", () => {
    assert.deepStrictEqual(inboxFilterQueryOpts("for-me", null), {
      pendingForMe: true,
      expanded: true,
    });
  });

  it("from-me composes --author with the broad pending predicate", () => {
    assert.deepStrictEqual(inboxFilterQueryOpts("from-me", "alice"), {
      author: "alice",
      pending: true,
      expanded: true,
    });
  });

  it("from-me is unavailable without a resolved identity", () => {
    assert.strictEqual(inboxFilterQueryOpts("from-me", null), null);
  });

  it("unassigned uses the broadcast filter and needs no client-side identity", () => {
    assert.deepStrictEqual(inboxFilterQueryOpts("unassigned", null), {
      pendingBroadcast: true,
      expanded: true,
    });
  });

  it("pending is the broad predicate alone", () => {
    assert.deepStrictEqual(inboxFilterQueryOpts("pending", "alice"), {
      pending: true,
      expanded: true,
    });
  });

  it("all applies no pending narrowing", () => {
    assert.deepStrictEqual(inboxFilterQueryOpts("all", "alice"), { expanded: true });
  });

  it("every mode requests expanded comments", () => {
    for (const option of INBOX_FILTER_OPTIONS) {
      assert.strictEqual(inboxFilterQueryOpts(option.value, "alice")?.expanded, true, option.value);
    }
  });

  it("no mode mixes two pending flavors in one query", () => {
    for (const option of INBOX_FILTER_OPTIONS) {
      const opts = inboxFilterQueryOpts(option.value, "alice");
      const flavors = [opts?.pending, opts?.pendingForMe, opts?.pendingBroadcast].filter(Boolean);
      assert.ok(flavors.length <= 1, `${option.value} set ${flavors.length} pending flavors`);
    }
  });
});

describe("inboxEmptyMessage", () => {
  it("gives every mode its own copy", () => {
    const messages = INBOX_FILTER_OPTIONS.map((o) => inboxEmptyMessage(o.value));
    assert.deepStrictEqual(messages, [
      "Nothing pending for you.",
      "Nothing waiting on others.",
      "No unassigned pending comments.",
      "No pending comments.",
      "No comments found.",
    ]);
    assert.strictEqual(new Set(messages).size, messages.length);
  });
});

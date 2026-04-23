import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { ComponentCheck } from "@/lib/githubReleases.ts";
import {
  canUpdatePlugin,
  messageForCheck,
  messageForUpdate,
  statusChipClasses,
  statusLabel,
  tailText,
} from "./updatesSection.helpers.ts";

/**
 * The UpdatesSection component is driven by four pure helpers. Covering
 * them individually is enough to prove every render branch — the
 * component's JSX is a thin shell over these functions, and wiring
 * React + jsdom into node's strip-only test loader would add more risk
 * than coverage for branches this deterministic.
 */

const check = (overrides: Partial<ComponentCheck> = {}): ComponentCheck => ({
  status: "up-to-date",
  installed: "0.1.6",
  latest: null,
  tag: null,
  releaseUrl: null,
  ...overrides,
});

describe("statusLabel", () => {
  it("labels up-to-date", () => {
    assert.strictEqual(statusLabel(check()), "up to date");
  });

  it("labels update-available", () => {
    assert.strictEqual(statusLabel(check({ status: "update-available" })), "update available");
  });

  it("labels check-failed", () => {
    assert.strictEqual(statusLabel(check({ status: "check-failed" })), "check failed");
  });
});

describe("statusChipClasses", () => {
  it("is accent-filled for update-available", () => {
    const cls = statusChipClasses(check({ status: "update-available" }));
    assert.match(cls, /bg-accent/);
    assert.match(cls, /text-white/);
  });

  it("is red-tinted for check-failed", () => {
    const cls = statusChipClasses(check({ status: "check-failed" }));
    assert.match(cls, /text-red-400/);
  });

  it("is muted for up-to-date", () => {
    const cls = statusChipClasses(check());
    assert.match(cls, /text-text-muted/);
  });
});

describe("tailText", () => {
  it("returns the original string when under the cap", () => {
    assert.strictEqual(tailText("short message"), "short message");
  });

  it("trims surrounding whitespace", () => {
    assert.strictEqual(tailText("  hi\n"), "hi");
  });

  it("tails long strings with an ellipsis prefix", () => {
    const long = "x".repeat(250);
    const result = tailText(long, 50);
    assert.strictEqual(result.length, 51);
    assert.ok(result.startsWith("…"));
    assert.strictEqual(result.slice(1), "x".repeat(50));
  });
});

describe("canUpdatePlugin", () => {
  it("is false when the check is missing", () => {
    assert.strictEqual(canUpdatePlugin(undefined), false);
  });

  it("is true only when status === 'update-available'", () => {
    assert.strictEqual(canUpdatePlugin(check({ status: "update-available" })), true);
    assert.strictEqual(canUpdatePlugin(check()), false);
    assert.strictEqual(canUpdatePlugin(check({ status: "check-failed" })), false);
  });
});

describe("messageForUpdate", () => {
  it("produces the reload-notice text on success", () => {
    const msg = messageForUpdate({ ok: true, stderr: "" });
    assert.ok(msg);
    assert.strictEqual(msg?.ok, true);
    assert.match(msg?.text ?? "", /reload Obsidian/);
  });

  it("echoes the stderr tail on CLI failure", () => {
    const msg = messageForUpdate({ ok: false, stderr: "unrecognized subcommand 'obsidian'" });
    assert.ok(msg);
    assert.strictEqual(msg?.ok, false);
    assert.match(msg?.text ?? "", /Update failed/);
    assert.match(msg?.text ?? "", /unrecognized subcommand/);
  });

  it("falls back to a generic message when stderr is empty", () => {
    const msg = messageForUpdate({ ok: false, stderr: "" });
    assert.strictEqual(msg?.text, "Update failed: unknown error");
  });

  it("handles thrown errors", () => {
    const msg = messageForUpdate(new Error("spawn ENOENT"));
    assert.strictEqual(msg?.ok, false);
    assert.match(msg?.text ?? "", /spawn ENOENT/);
  });
});

describe("messageForCheck", () => {
  it("produces a success banner when the refresh resolves", () => {
    const msg = messageForCheck();
    assert.deepStrictEqual(msg, { ok: true, text: "Checked." });
  });

  it("surfaces the thrown error message", () => {
    const msg = messageForCheck(new Error("offline"));
    assert.strictEqual(msg?.ok, false);
    assert.strictEqual(msg?.text, "offline");
  });

  it("falls back to a generic string when the error has no message", () => {
    const msg = messageForCheck(new Error(""));
    assert.strictEqual(msg?.text, "Check failed");
  });
});

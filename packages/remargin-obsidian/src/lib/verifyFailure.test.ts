import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { parseVerifyFailure } from "./verifyFailure.ts";

describe("parseVerifyFailure", () => {
  it("returns null for non-verify errors", () => {
    assert.strictEqual(parseVerifyFailure("connection refused"), null);
  });

  it("returns null for non-Error / non-string inputs", () => {
    assert.strictEqual(parseVerifyFailure(null), null);
    assert.strictEqual(parseVerifyFailure({ kind: "other" }), null);
  });

  it("returns null when the JSON does not match the verify_failed shape", () => {
    const payload = JSON.stringify({ error_kind: "permissions_denied" });
    assert.strictEqual(parseVerifyFailure(new Error(payload)), null);
  });

  it("parses the verify_failed JSON shape from a CLI stderr blob", () => {
    const payload = JSON.stringify({
      elapsed_ms: 12,
      error_kind: "verify_failed",
      failures: [
        { checksum_ok: true, id: "abc", recipients: "ok", signature: "missing" },
        { checksum_ok: true, id: "def", recipients: "ok", signature: "missing" },
      ],
      headline: "verify failed: 2 unsigned or invalid comments in /d/a.md",
      hint: "Try `remargin verify /d/a.md --json` for the full breakdown.",
      mode: "strict",
      path: "/d/a.md",
    });
    const message = `${payload}\n  command: remargin ack --file /d/a.md abc`;
    const parsed = parseVerifyFailure(new Error(message));
    assert.ok(parsed !== null, "should parse the verify_failed shape");
    assert.strictEqual(parsed?.error_kind, "verify_failed");
    assert.strictEqual(parsed?.failures.length, 2);
    assert.strictEqual(parsed?.path, "/d/a.md");
    assert.strictEqual(parsed?.mode, "strict");
    assert.match(parsed?.headline ?? "", /^verify failed:/u);
  });

  it("ignores leading text before the JSON object", () => {
    const payload = JSON.stringify({
      error_kind: "verify_failed",
      failures: [{ checksum_ok: false, id: "abc", recipients: "ok", signature: "missing" }],
      headline: "verify failed: 1 unsigned or invalid comment in /d/a.md",
      hint: "Try ...",
      mode: "open",
      path: "/d/a.md",
    });
    const message = `error: ${payload}`;
    const parsed = parseVerifyFailure(new Error(message));
    assert.ok(parsed !== null);
    assert.strictEqual(parsed?.failures[0]?.id, "abc");
  });
});

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { formatRelative } from "./relative-time.ts";

describe("formatRelative", () => {
  const now = new Date("2026-04-12T12:00:00Z");

  it("returns empty string for empty input", () => {
    assert.strictEqual(formatRelative(undefined, now), "");
    assert.strictEqual(formatRelative("", now), "");
  });

  it("returns empty string for invalid input", () => {
    assert.strictEqual(formatRelative("not-a-date", now), "");
  });

  it("returns 'now' for under-a-minute diffs", () => {
    const ts = new Date(now.getTime() - 30 * 1000).toISOString();
    assert.strictEqual(formatRelative(ts, now), "now");
  });

  it("returns minutes for sub-hour diffs", () => {
    const ts = new Date(now.getTime() - 5 * 60 * 1000).toISOString();
    assert.strictEqual(formatRelative(ts, now), "5m");
  });

  it("returns hours for sub-day diffs", () => {
    const ts = new Date(now.getTime() - 2 * 60 * 60 * 1000).toISOString();
    assert.strictEqual(formatRelative(ts, now), "2h");
  });

  it("returns days for diffs under a week", () => {
    const ts = new Date(now.getTime() - 3 * 24 * 60 * 60 * 1000).toISOString();
    assert.strictEqual(formatRelative(ts, now), "3d");
  });

  it("returns month + day for older entries", () => {
    const result = formatRelative("2026-03-20T12:00:00Z", now);
    assert.match(result, /^[A-Z][a-z]{2} \d{1,2}$/);
  });

  it("accepts Date instances as input", () => {
    const ts = new Date(now.getTime() - 60 * 60 * 1000);
    assert.strictEqual(formatRelative(ts, now), "1h");
  });

  it("treats future timestamps as 'now'", () => {
    const ts = new Date(now.getTime() + 60 * 1000).toISOString();
    assert.strictEqual(formatRelative(ts, now), "now");
  });
});

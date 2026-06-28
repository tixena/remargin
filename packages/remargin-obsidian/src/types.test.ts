import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import {
  clampMarkdownScale,
  DEFAULT_SETTINGS,
  MARKDOWN_SCALE_DEFAULT,
  MARKDOWN_SCALE_MAX,
  MARKDOWN_SCALE_MIN,
  type RemarginSettings,
} from "./types.ts";

describe("RemarginSettings shape", () => {
  // Test #14 (T36 spec): editorWidgets defaults to false. Importing the
  // type alongside DEFAULT_SETTINGS doubles as a compile-time check that
  // the field is part of the shape.
  it("DEFAULT_SETTINGS.editorWidgets is false", () => {
    const settings: RemarginSettings = DEFAULT_SETTINGS;
    assert.equal(settings.editorWidgets, false);
  });

  it("DEFAULT_SETTINGS.markdownScale is the default scale", () => {
    assert.equal(DEFAULT_SETTINGS.markdownScale, MARKDOWN_SCALE_DEFAULT);
  });
});

describe("clampMarkdownScale", () => {
  it("clamps below the minimum up to the floor", () => {
    assert.equal(clampMarkdownScale(0.1), MARKDOWN_SCALE_MIN);
  });

  it("clamps above the maximum down to the ceiling", () => {
    assert.equal(clampMarkdownScale(5), MARKDOWN_SCALE_MAX);
  });

  it("snaps float drift to two decimals", () => {
    assert.equal(clampMarkdownScale(1.0000000001), 1);
    assert.equal(clampMarkdownScale(1.2999999999), 1.3);
  });

  it("falls back to the default for non-finite input", () => {
    assert.equal(clampMarkdownScale(Number.NaN), MARKDOWN_SCALE_DEFAULT);
    assert.equal(clampMarkdownScale(Number.POSITIVE_INFINITY), MARKDOWN_SCALE_DEFAULT);
  });
});

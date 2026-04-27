import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { DEFAULT_SETTINGS, type RemarginSettings } from "./types.ts";

describe("RemarginSettings shape", () => {
  // Test #14 (T36 spec): editorWidgets defaults to false. Importing the
  // type alongside DEFAULT_SETTINGS doubles as a compile-time check that
  // the field is part of the shape.
  it("DEFAULT_SETTINGS.editorWidgets is false", () => {
    const settings: RemarginSettings = DEFAULT_SETTINGS;
    assert.equal(settings.editorWidgets, false);
  });
});

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { identityShort } from "./identity-hash.ts";

describe("identityShort", () => {
  it("returns empty string for empty input", () => {
    assert.strictEqual(identityShort(""), "");
    assert.strictEqual(identityShort(undefined), "");
    assert.strictEqual(identityShort(null), "");
    assert.strictEqual(identityShort("   "), "");
  });

  it("returns first three chars for long hex-like identity strings", () => {
    assert.strictEqual(identityShort("xws123abcd"), "xws");
    assert.strictEqual(identityShort("PVO9876543"), "pvo");
  });

  it("derives a three-character shortcode for short names", () => {
    const code = identityShort("Eduardo");
    assert.strictEqual(code.length, 3);
    assert.match(code, /^[a-z0-9]+$/);
  });

  it("is deterministic", () => {
    assert.strictEqual(identityShort("claude"), identityShort("claude"));
    assert.strictEqual(identityShort("eduardo"), identityShort("Eduardo"));
  });

  it("produces different codes for different inputs", () => {
    assert.notStrictEqual(identityShort("alice"), identityShort("bob"));
  });
});

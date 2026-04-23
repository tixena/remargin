import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { compareVersions, isNewer, parseVersion } from "./semver.ts";

describe("parseVersion", () => {
  it("parses plain dotted triples", () => {
    assert.deepStrictEqual(parseVersion("1.2.3"), {
      major: 1,
      minor: 2,
      patch: 3,
      prerelease: undefined,
    });
  });

  it("strips a leading v", () => {
    assert.deepStrictEqual(parseVersion("v0.1.6"), {
      major: 0,
      minor: 1,
      patch: 6,
      prerelease: undefined,
    });
  });

  it("extracts from a prefixed git tag", () => {
    assert.deepStrictEqual(parseVersion("obsidian-v0.1.6"), {
      major: 0,
      minor: 1,
      patch: 6,
      prerelease: undefined,
    });
  });

  it("captures a prerelease suffix", () => {
    assert.deepStrictEqual(parseVersion("0.4.0-rc.1"), {
      major: 0,
      minor: 4,
      patch: 0,
      prerelease: "rc.1",
    });
  });

  it("pulls the version out of `remargin X.Y.Z` output", () => {
    const parsed = parseVersion("remargin 0.4.2\n");
    assert.deepStrictEqual(parsed, {
      major: 0,
      minor: 4,
      patch: 2,
      prerelease: undefined,
    });
  });

  it("returns null when no triple is present", () => {
    assert.strictEqual(parseVersion(""), null);
    assert.strictEqual(parseVersion("not-a-version"), null);
    assert.strictEqual(parseVersion("v1.2"), null);
  });
});

describe("compareVersions", () => {
  const v = (s: string) => {
    const parsed = parseVersion(s);
    assert.ok(parsed, `expected ${s} to parse`);
    return parsed;
  };

  it("orders by major, then minor, then patch", () => {
    assert.ok(compareVersions(v("1.0.0"), v("0.9.9")) > 0);
    assert.ok(compareVersions(v("0.2.0"), v("0.1.9")) > 0);
    assert.ok(compareVersions(v("0.1.2"), v("0.1.1")) > 0);
  });

  it("returns zero for equal versions", () => {
    assert.strictEqual(compareVersions(v("0.1.6"), v("v0.1.6")), 0);
  });

  it("sorts stable above prerelease of the same triple", () => {
    assert.ok(compareVersions(v("0.4.0"), v("0.4.0-rc.1")) > 0);
    assert.ok(compareVersions(v("0.4.0-rc.1"), v("0.4.0")) < 0);
  });

  it("falls back to string order for two prereleases", () => {
    assert.ok(compareVersions(v("0.4.0-rc.2"), v("0.4.0-rc.1")) > 0);
  });
});

describe("isNewer", () => {
  it("is true when latest strictly outranks installed", () => {
    assert.strictEqual(isNewer("0.1.7", "0.1.6"), true);
    assert.strictEqual(isNewer("obsidian-v0.2.0", "v0.1.9"), true);
  });

  it("is false when equal", () => {
    assert.strictEqual(isNewer("0.1.6", "0.1.6"), false);
  });

  it("is false when installed outranks latest", () => {
    assert.strictEqual(isNewer("0.1.5", "0.1.6"), false);
  });

  it("is false when either version is unparseable", () => {
    assert.strictEqual(isNewer("garbage", "0.1.6"), false);
    assert.strictEqual(isNewer("0.1.7", "garbage"), false);
  });
});

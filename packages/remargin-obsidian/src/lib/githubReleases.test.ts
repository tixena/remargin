import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import {
  classifyTag,
  compareComponent,
  detectNewUpdates,
  type GithubRelease,
  isCacheFresh,
  type ReleasesFetcher,
  runUpdateCheck,
  splitReleases,
  type UpdateCheckState,
} from "./githubReleases.ts";

describe("classifyTag", () => {
  it("recognises plugin tags", () => {
    assert.strictEqual(classifyTag("obsidian-v0.1.6"), "plugin");
  });

  it("recognises CLI tags", () => {
    assert.strictEqual(classifyTag("v0.4.2"), "cli");
  });

  it("rejects empty or unrelated tags", () => {
    assert.strictEqual(classifyTag(""), null);
    assert.strictEqual(classifyTag("release-notes"), null);
    assert.strictEqual(classifyTag("nightly"), null);
  });
});

describe("splitReleases", () => {
  const rel = (tag: string, published?: string, extras: Partial<GithubRelease> = {}): GithubRelease => ({
    tag_name: tag,
    published_at: published ?? null,
    ...extras,
  });

  it("returns the newest plugin and CLI release", () => {
    const releases: GithubRelease[] = [
      rel("obsidian-v0.1.5", "2026-03-01T00:00:00Z"),
      rel("obsidian-v0.1.6", "2026-04-01T00:00:00Z"),
      rel("v0.4.1", "2026-02-01T00:00:00Z"),
      rel("v0.4.2", "2026-04-10T00:00:00Z"),
      rel("untagged-thing", "2026-04-11T00:00:00Z"),
    ];
    const { plugin, cli } = splitReleases(releases);
    assert.strictEqual(plugin?.tag_name, "obsidian-v0.1.6");
    assert.strictEqual(cli?.tag_name, "v0.4.2");
  });

  it("skips drafts and prereleases", () => {
    const releases: GithubRelease[] = [
      rel("obsidian-v0.2.0", "2026-04-20T00:00:00Z", { prerelease: true }),
      rel("obsidian-v0.1.7", "2026-04-18T00:00:00Z", { draft: true }),
      rel("obsidian-v0.1.6", "2026-04-01T00:00:00Z"),
    ];
    const { plugin } = splitReleases(releases);
    assert.strictEqual(plugin?.tag_name, "obsidian-v0.1.6");
  });

  it("handles an empty release list", () => {
    const { plugin, cli } = splitReleases([]);
    assert.strictEqual(plugin, null);
    assert.strictEqual(cli, null);
  });

  it("falls back to input order when timestamps are missing", () => {
    const releases: GithubRelease[] = [
      rel("obsidian-v0.1.6"),
      rel("obsidian-v0.1.5"),
    ];
    const { plugin } = splitReleases(releases);
    assert.strictEqual(plugin?.tag_name, "obsidian-v0.1.6");
  });
});

describe("compareComponent", () => {
  it("flags an update when latest outranks installed", () => {
    const result = compareComponent("0.1.5", {
      tag_name: "obsidian-v0.1.6",
      html_url: "https://github.com/tixena/remargin/releases/tag/obsidian-v0.1.6",
      name: "Remargin plugin 0.1.6",
    });
    assert.strictEqual(result.status, "update-available");
    assert.strictEqual(result.latest, "Remargin plugin 0.1.6");
    assert.strictEqual(result.tag, "obsidian-v0.1.6");
  });

  it("is up-to-date when installed matches latest", () => {
    const result = compareComponent("0.1.6", { tag_name: "obsidian-v0.1.6" });
    assert.strictEqual(result.status, "up-to-date");
  });

  it("is up-to-date with null latest when no release exists", () => {
    const result = compareComponent("0.4.2", null);
    assert.strictEqual(result.status, "up-to-date");
    assert.strictEqual(result.latest, null);
  });

  it("reports check-failed when installed is unknown", () => {
    const result = compareComponent("unknown", { tag_name: "v0.4.2" });
    assert.strictEqual(result.status, "check-failed");
  });
});

describe("runUpdateCheck", () => {
  const releasesPayload = JSON.stringify([
    {
      tag_name: "obsidian-v0.1.6",
      published_at: "2026-04-01T00:00:00Z",
      html_url: "https://github.com/tixena/remargin/releases/tag/obsidian-v0.1.6",
    },
    {
      tag_name: "v0.4.2",
      published_at: "2026-04-10T00:00:00Z",
      html_url: "https://github.com/tixena/remargin/releases/tag/v0.4.2",
    },
  ]);

  const stubFetcher = (body: string, status = 200): ReleasesFetcher =>
    async () => ({ ok: status >= 200 && status < 300, status, body });

  it("produces per-component status from a real payload", async () => {
    const state = await runUpdateCheck({
      installedPlugin: "0.1.5",
      installedCli: "0.4.2",
      fetcher: stubFetcher(releasesPayload),
      now: () => new Date("2026-04-22T12:00:00Z"),
    });
    assert.strictEqual(state.plugin.status, "update-available");
    assert.strictEqual(state.plugin.tag, "obsidian-v0.1.6");
    assert.strictEqual(state.cli.status, "up-to-date");
    assert.strictEqual(state.lastCheckedAt, "2026-04-22T12:00:00.000Z");
  });

  it("folds HTTP errors into check-failed (does not throw)", async () => {
    const state = await runUpdateCheck({
      installedPlugin: "0.1.5",
      installedCli: "0.4.2",
      fetcher: stubFetcher("", 503),
      now: () => new Date("2026-04-22T12:00:00Z"),
    });
    assert.strictEqual(state.plugin.status, "check-failed");
    assert.strictEqual(state.cli.status, "check-failed");
    assert.match(state.plugin.error ?? "", /503/);
  });

  it("folds fetcher exceptions into check-failed", async () => {
    const throwing: ReleasesFetcher = async () => {
      throw new Error("offline");
    };
    const state = await runUpdateCheck({
      installedPlugin: "0.1.5",
      installedCli: "0.4.2",
      fetcher: throwing,
      now: () => new Date("2026-04-22T12:00:00Z"),
    });
    assert.strictEqual(state.plugin.status, "check-failed");
    assert.strictEqual(state.plugin.error, "offline");
  });

  it("handles a non-array JSON payload gracefully", async () => {
    const state = await runUpdateCheck({
      installedPlugin: "0.1.5",
      installedCli: "0.4.2",
      fetcher: stubFetcher('{"message":"rate limit"}'),
      now: () => new Date("2026-04-22T12:00:00Z"),
    });
    assert.strictEqual(state.plugin.status, "check-failed");
  });
});

describe("isCacheFresh", () => {
  const ok = (hoursAgo: number): UpdateCheckState => ({
    plugin: {
      status: "up-to-date",
      installed: "0.1.6",
      latest: null,
      tag: null,
      releaseUrl: null,
    },
    cli: {
      status: "up-to-date",
      installed: "0.4.2",
      latest: null,
      tag: null,
      releaseUrl: null,
    },
    lastCheckedAt: new Date(Date.now() - hoursAgo * 60 * 60 * 1000).toISOString(),
  });

  it("is false when no cache exists", () => {
    assert.strictEqual(isCacheFresh(null), false);
    assert.strictEqual(isCacheFresh(undefined), false);
  });

  it("is true when the cache is less than 24h old", () => {
    const now = new Date();
    const state = ok(1);
    assert.strictEqual(isCacheFresh(state, now), true);
  });

  it("is false when the cache is older than 24h", () => {
    const now = new Date();
    const state = ok(25);
    assert.strictEqual(isCacheFresh(state, now), false);
  });

  it("is false when either component is check-failed", () => {
    const now = new Date();
    const state = ok(1);
    state.plugin.status = "check-failed";
    assert.strictEqual(isCacheFresh(state, now), false);
  });

  it("is false when the timestamp is garbage", () => {
    const now = new Date();
    const state = ok(1);
    state.lastCheckedAt = "not-a-date";
    assert.strictEqual(isCacheFresh(state, now), false);
  });
});

describe("detectNewUpdates", () => {
  const baseCheck = (overrides: Partial<UpdateCheckState["plugin"]> = {}): UpdateCheckState["plugin"] => ({
    status: "up-to-date",
    installed: "0.1.6",
    latest: null,
    tag: null,
    releaseUrl: null,
    ...overrides,
  });

  const snapshot = (overrides: Partial<UpdateCheckState> = {}): UpdateCheckState => ({
    plugin: baseCheck(),
    cli: baseCheck({ installed: "0.4.2" }),
    lastCheckedAt: "2026-04-22T12:00:00Z",
    ...overrides,
  });

  it("reports a freshly discovered update for each component", () => {
    const after = snapshot({
      plugin: baseCheck({ status: "update-available", tag: "obsidian-v0.1.7", latest: "0.1.7" }),
    });
    assert.deepStrictEqual(detectNewUpdates(null, after), ["plugin"]);
  });

  it("does not refire when the same release is already flagged", () => {
    const before = snapshot({
      plugin: baseCheck({ status: "update-available", tag: "obsidian-v0.1.7", latest: "0.1.7" }),
    });
    const after = snapshot({
      plugin: baseCheck({ status: "update-available", tag: "obsidian-v0.1.7", latest: "0.1.7" }),
    });
    assert.deepStrictEqual(detectNewUpdates(before, after), []);
  });

  it("refires when the latest tag advances to a newer release", () => {
    const before = snapshot({
      plugin: baseCheck({ status: "update-available", tag: "obsidian-v0.1.7", latest: "0.1.7" }),
    });
    const after = snapshot({
      plugin: baseCheck({ status: "update-available", tag: "obsidian-v0.1.8", latest: "0.1.8" }),
    });
    assert.deepStrictEqual(detectNewUpdates(before, after), ["plugin"]);
  });

  it("fires per-component independently", () => {
    const before = snapshot();
    const after = snapshot({
      plugin: baseCheck({ status: "update-available", tag: "obsidian-v0.1.7", latest: "0.1.7" }),
      cli: baseCheck({ installed: "0.4.2", status: "update-available", tag: "v0.5.0", latest: "0.5.0" }),
    });
    assert.deepStrictEqual(detectNewUpdates(before, after).sort(), ["cli", "plugin"]);
  });
});

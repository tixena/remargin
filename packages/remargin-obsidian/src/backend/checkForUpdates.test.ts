import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { ReleasesFetcher, UpdateCheckState } from "@/lib/githubReleases.ts";
import { performUpdateCheck } from "./performUpdateCheck.ts";

/**
 * These tests cover the standalone `performUpdateCheck` helper that
 * backs `RemarginBackend.checkForUpdates`. The helper is extracted from
 * the backend class so the strip-only TypeScript loader used by node's
 * test runner can parse it — the class's parameter-property constructor
 * is not valid input for the loader, which is why every
 * backend-adjacent unit test imports helpers directly instead of
 * instantiating `RemarginBackend`.
 */

const freshCache = (): UpdateCheckState => ({
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
  lastCheckedAt: new Date().toISOString(),
});

describe("performUpdateCheck", () => {
  it("returns the cached snapshot without calling the fetcher when fresh", async () => {
    let fetchCalls = 0;
    let cliCalls = 0;
    const fetcher: ReleasesFetcher = async () => {
      fetchCalls += 1;
      return { ok: true, status: 200, body: "[]" };
    };
    const cache = freshCache();
    const result = await performUpdateCheck({
      force: false,
      installedPlugin: "0.1.6",
      fetcher,
      cache,
      cliVersion: async () => {
        cliCalls += 1;
        return "remargin 0.4.2";
      },
    });
    assert.strictEqual(fetchCalls, 0);
    assert.strictEqual(cliCalls, 0);
    assert.strictEqual(result, cache);
  });

  it("forces a refetch when force=true even with a fresh cache", async () => {
    let fetchCalls = 0;
    const fetcher: ReleasesFetcher = async () => {
      fetchCalls += 1;
      return {
        ok: true,
        status: 200,
        body: JSON.stringify([
          {
            tag_name: "obsidian-v0.1.7",
            published_at: "2026-04-22T00:00:00Z",
            html_url: "https://example.com/plugin",
          },
          {
            tag_name: "v0.5.0",
            published_at: "2026-04-22T00:00:00Z",
            html_url: "https://example.com/cli",
          },
        ]),
      };
    };
    const result = await performUpdateCheck({
      force: true,
      installedPlugin: "0.1.6",
      fetcher,
      cache: freshCache(),
      cliVersion: async () => "remargin 0.4.2",
      now: () => new Date("2026-04-22T12:00:00Z"),
    });
    assert.strictEqual(fetchCalls, 1);
    assert.strictEqual(result.plugin.status, "update-available");
    assert.strictEqual(result.plugin.tag, "obsidian-v0.1.7");
    assert.strictEqual(result.cli.status, "update-available");
    assert.strictEqual(result.cli.tag, "v0.5.0");
  });

  it("refetches when the cache is stale", async () => {
    const stale: UpdateCheckState = {
      ...freshCache(),
      lastCheckedAt: new Date(Date.now() - 2 * 24 * 60 * 60 * 1000).toISOString(),
    };
    let fetchCalls = 0;
    const fetcher: ReleasesFetcher = async () => {
      fetchCalls += 1;
      return { ok: true, status: 200, body: "[]" };
    };
    await performUpdateCheck({
      force: false,
      installedPlugin: "0.1.6",
      fetcher,
      cache: stale,
      cliVersion: async () => "remargin 0.4.2",
    });
    assert.strictEqual(fetchCalls, 1);
  });

  it("folds a CLI-probe failure into check-failed without throwing", async () => {
    const fetcher: ReleasesFetcher = async () => ({
      ok: true,
      status: 200,
      body: "[]",
    });
    const result = await performUpdateCheck({
      force: true,
      installedPlugin: "0.1.6",
      fetcher,
      cliVersion: async () => {
        throw new Error("binary missing");
      },
      now: () => new Date("2026-04-22T12:00:00Z"),
    });
    assert.strictEqual(result.cli.status, "check-failed");
    assert.strictEqual(result.plugin.status, "up-to-date");
  });
});

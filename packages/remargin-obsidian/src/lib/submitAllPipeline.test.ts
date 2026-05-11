import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type {
  StagedGroup,
  SubmitGroupResult,
} from "../components/sidebar/buildPromptGroups.ts";
import { runSubmitAll } from "./submitAllPipeline.ts";

function makeGroup(name: string, files: string[]): StagedGroup {
  return {
    prompt: { is_default: false, name, prompt: `body for ${name}`, source: null },
    files,
  };
}

describe("runSubmitAll", () => {
  it("returns empty results for an empty group list", async () => {
    const results = await runSubmitAll({
      groups: [],
      runGroup: async () => {},
      cleanupGroup: async () => {},
    });
    assert.deepEqual(results, []);
  });

  it("runs all groups sequentially and marks them ok", async () => {
    const order: string[] = [];
    const groups = [makeGroup("A", ["a.md"]), makeGroup("B", ["b.md"]), makeGroup("C", ["c.md"])];
    const results = await runSubmitAll({
      groups,
      runGroup: async (g) => {
        order.push(`run:${g.prompt.name}`);
      },
      cleanupGroup: async (g) => {
        order.push(`clean:${g.prompt.name}`);
      },
    });
    assert.deepEqual(order, [
      "run:A",
      "clean:A",
      "run:B",
      "clean:B",
      "run:C",
      "clean:C",
    ]);
    assert.equal(results.every((r) => r.ok), true);
  });

  it("continues past a middle failure and preserves order", async () => {
    const groups = [makeGroup("A", ["a"]), makeGroup("B", ["b"]), makeGroup("C", ["c"])];
    const cleaned: string[] = [];
    const results = await runSubmitAll({
      groups,
      runGroup: async (g) => {
        if (g.prompt.name === "B") throw new Error("boom");
      },
      cleanupGroup: async (g) => {
        cleaned.push(g.prompt.name);
      },
    });
    assert.deepEqual(cleaned, ["A", "C"]);
    assert.equal(results[0]?.ok, true);
    assert.equal(results[1]?.ok, false);
    assert.equal(results[1]?.error, "boom");
    assert.equal(results[2]?.ok, true);
  });

  it("calls bumpRefresh after each successful group", async () => {
    let bumps = 0;
    const groups = [makeGroup("A", ["a"]), makeGroup("B", ["b"])];
    await runSubmitAll({
      groups,
      runGroup: async () => {},
      cleanupGroup: async () => {},
      bumpRefresh: () => {
        bumps += 1;
      },
    });
    assert.equal(bumps, 2);
  });

  it("does not call bumpRefresh for failed groups", async () => {
    let bumps = 0;
    const groups = [makeGroup("A", ["a"]), makeGroup("B", ["b"])];
    await runSubmitAll({
      groups,
      runGroup: async (g) => {
        if (g.prompt.name === "A") throw new Error("nope");
      },
      cleanupGroup: async () => {},
      bumpRefresh: () => {
        bumps += 1;
      },
    });
    assert.equal(bumps, 1);
  });

  it("fires progress callbacks in pairs and in order", async () => {
    const events: string[] = [];
    const groups = [makeGroup("A", ["a"]), makeGroup("B", ["b"])];
    await runSubmitAll({
      groups,
      runGroup: async () => {},
      cleanupGroup: async () => {},
      progress: {
        onGroupStart: (g) => events.push(`start:${g.prompt.name}`),
        onGroupComplete: (g, r) => events.push(`done:${g.prompt.name}:${r.ok}`),
      },
    });
    assert.deepEqual(events, ["start:A", "done:A:true", "start:B", "done:B:true"]);
  });

  it("treats cleanup failure as ok=true with a warning", async () => {
    const groups = [makeGroup("A", ["a"])];
    const events: string[] = [];
    const results: SubmitGroupResult[] = await runSubmitAll({
      groups,
      runGroup: async () => {},
      cleanupGroup: async () => {
        throw new Error("disk full");
      },
      progress: {
        onGroupComplete: (g, r) => events.push(`done:${g.prompt.name}:${r.ok}:${r.error}`),
      },
    });
    assert.equal(results[0]?.ok, true);
    assert.ok(results[0]?.error?.includes("cleanup failed"));
    assert.deepEqual(events, ["done:A:true:disk full"]);
  });

  it("does not call cleanup when runGroup rejects", async () => {
    let cleanupCalls = 0;
    const groups = [makeGroup("A", ["a"])];
    await runSubmitAll({
      groups,
      runGroup: async () => {
        throw new Error("nope");
      },
      cleanupGroup: async () => {
        cleanupCalls += 1;
      },
    });
    assert.equal(cleanupCalls, 0);
  });

  it("captures durationMs per group", async () => {
    let t = 100;
    const groups = [makeGroup("A", ["a"])];
    const results = await runSubmitAll({
      groups,
      runGroup: async () => {},
      cleanupGroup: async () => {},
      now: () => {
        const v = t;
        t += 42;
        return v;
      },
    });
    assert.equal(results[0]?.durationMs, 42);
  });
});

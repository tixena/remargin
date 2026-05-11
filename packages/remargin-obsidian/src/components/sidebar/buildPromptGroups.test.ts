import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { ResolvedSystemPrompt } from "../../backend/types.ts";
import { buildPromptGroups } from "./buildPromptGroups.ts";

function explicit(name: string, source: string): ResolvedSystemPrompt {
  return { is_default: false, name, prompt: `body for ${name}`, source };
}

function deflt(name = "default"): ResolvedSystemPrompt {
  return {
    is_default: true,
    name,
    prompt: "Please process the comments in <files> using the remargin skill",
    source: null,
  };
}

describe("buildPromptGroups", () => {
  it("returns empty for an empty file list", () => {
    const out = buildPromptGroups([], new Map(), new Map(), new Set());
    assert.equal(out.length, 0);
  });

  it("buckets all files under one Default group when every file resolves to default", () => {
    const files = ["a.md", "b.md", "c.md"];
    const prompts = new Map<string, ResolvedSystemPrompt>(files.map((f) => [f, deflt()]));
    const out = buildPromptGroups(files, prompts, new Map(), new Set(files));
    assert.equal(out.length, 1);
    assert.equal(out[0]?.isDefault, true);
    assert.deepEqual(out[0]?.files, files);
    assert.deepEqual(out[0]?.staged, files);
    assert.deepEqual(out[0]?.unstaged, []);
    assert.equal(out[0]?.scope, "(vault)");
  });

  it("groups files by resolved source and lex-sorts the explicit groups", () => {
    const swe = explicit("SWE reviewer", "/vault/code/.remargin.yaml");
    const docs = explicit("Docs", "/vault/docs/.remargin.yaml");
    const files = ["code/a.md", "docs/b.md", "code/c.md"];
    const prompts = new Map<string, ResolvedSystemPrompt>([
      ["code/a.md", swe],
      ["docs/b.md", docs],
      ["code/c.md", swe],
    ]);
    const out = buildPromptGroups(files, prompts, new Map(), new Set(files));
    assert.equal(out.length, 2);
    // /vault/code comes before /vault/docs lex-wise.
    assert.equal(out[0]?.name, "SWE reviewer");
    assert.deepEqual(out[0]?.files, ["code/a.md", "code/c.md"]);
    assert.equal(out[1]?.name, "Docs");
  });

  it("places the Default group last after explicit ones", () => {
    const swe = explicit("SWE reviewer", "/vault/code/.remargin.yaml");
    const files = ["a.md", "code/b.md", "c.md"];
    const prompts = new Map<string, ResolvedSystemPrompt>([
      ["a.md", deflt()],
      ["code/b.md", swe],
      ["c.md", deflt()],
    ]);
    const out = buildPromptGroups(files, prompts, new Map(), new Set());
    assert.equal(out.length, 2);
    assert.equal(out[0]?.name, "SWE reviewer");
    assert.equal(out[1]?.isDefault, true);
    assert.deepEqual(out[1]?.files, ["a.md", "c.md"]);
  });

  it("splits staged vs unstaged inside each group from the staged set", () => {
    const swe = explicit("SWE reviewer", "/vault/code/.remargin.yaml");
    const files = ["a.md", "b.md"];
    const prompts = new Map<string, ResolvedSystemPrompt>([
      ["a.md", swe],
      ["b.md", swe],
    ]);
    const out = buildPromptGroups(files, prompts, new Map(), new Set(["a.md"]));
    assert.equal(out.length, 1);
    assert.deepEqual(out[0]?.staged, ["a.md"]);
    assert.deepEqual(out[0]?.unstaged, ["b.md"]);
  });

  it("collects resolver failures into a synthetic error group placed above Default", () => {
    const files = ["bad.md", "ok.md"];
    const prompts = new Map<string, ResolvedSystemPrompt>([["ok.md", deflt()]]);
    const errors = new Map<string, string>([["bad.md", "walk-up failed"]]);
    const out = buildPromptGroups(files, prompts, errors, new Set());
    assert.equal(out.length, 2);
    assert.equal(out[0]?.hasError, true);
    assert.equal(out[0]?.errorMessage, "walk-up failed");
    assert.equal(out[1]?.isDefault, true);
  });

  it("places error group above Default but after explicit groups", () => {
    const swe = explicit("SWE reviewer", "/vault/code/.remargin.yaml");
    const files = ["code/ok.md", "bad.md", "default.md"];
    const prompts = new Map<string, ResolvedSystemPrompt>([
      ["code/ok.md", swe],
      ["default.md", deflt()],
    ]);
    const errors = new Map<string, string>([["bad.md", "boom"]]);
    const out = buildPromptGroups(files, prompts, errors, new Set());
    assert.equal(out.length, 3);
    assert.equal(out[0]?.name, "SWE reviewer");
    assert.equal(out[1]?.hasError, true);
    assert.equal(out[2]?.isDefault, true);
  });

  it("buckets unresolved-yet files under a Default placeholder", () => {
    const files = ["a.md"];
    const out = buildPromptGroups(files, new Map(), new Map(), new Set());
    assert.equal(out.length, 1);
    assert.equal(out[0]?.isDefault, true);
    assert.deepEqual(out[0]?.files, ["a.md"]);
  });
});

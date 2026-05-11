import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { removeSystemPrompt, spliceSystemPrompt } from "./yamlSystemPrompt.ts";

describe("spliceSystemPrompt", () => {
  it("appends a new block at EOF when none exists", () => {
    const existing = "identity: alice\ntype: human\n";
    const out = spliceSystemPrompt(existing, { name: "SWE", prompt: "review" });
    assert.equal(
      out.content,
      "identity: alice\ntype: human\n\nsystem_prompt:\n  name: SWE\n  prompt: review\n"
    );
    assert.equal(out.noop, false);
  });

  it("appends without a name when name is omitted", () => {
    const existing = "identity: alice\n";
    const out = spliceSystemPrompt(existing, { prompt: "review" });
    assert.equal(out.content, "identity: alice\n\nsystem_prompt:\n  prompt: review\n");
  });

  it("replaces an existing block at end of file", () => {
    const existing = "identity: alice\nsystem_prompt:\n  name: old\n  prompt: old body\n";
    const out = spliceSystemPrompt(existing, { name: "new", prompt: "new body" });
    assert.equal(
      out.content,
      "identity: alice\nsystem_prompt:\n  name: new\n  prompt: new body\n"
    );
  });

  it("replaces a middle block and preserves trailing fields", () => {
    const existing =
      "identity: alice\nsystem_prompt:\n  name: old\n  prompt: old body\nmode: open\n";
    const out = spliceSystemPrompt(existing, { name: "new", prompt: "new body" });
    assert.equal(
      out.content,
      "identity: alice\nsystem_prompt:\n  name: new\n  prompt: new body\nmode: open\n"
    );
  });

  it("writes prompt: \"\" verbatim for an empty body", () => {
    const existing = "identity: alice\n";
    const out = spliceSystemPrompt(existing, { name: "n", prompt: "" });
    assert.ok(out.content.includes('prompt: ""'));
  });

  it("uses block scalar style for multi-line bodies", () => {
    const existing = "";
    const out = spliceSystemPrompt(existing, { name: "n", prompt: "line one\nline two" });
    assert.ok(out.content.includes("prompt: |"));
    assert.ok(out.content.includes("    line one"));
    assert.ok(out.content.includes("    line two"));
  });

  it("uses block scalar style for bodies with YAML-special characters", () => {
    const existing = "";
    const out = spliceSystemPrompt(existing, { prompt: "review : carefully" });
    assert.ok(out.content.includes("prompt: |"), out.content);
    assert.ok(out.content.includes("    review : carefully"));
  });

  it("returns noop when the new block matches the existing one", () => {
    const existing = "system_prompt:\n  name: n\n  prompt: body\n";
    const out = spliceSystemPrompt(existing, { name: "n", prompt: "body" });
    assert.equal(out.noop, true);
    assert.equal(out.content, existing);
  });

  it("appends to an empty file", () => {
    const out = spliceSystemPrompt("", { name: "n", prompt: "body" });
    assert.equal(out.content, "system_prompt:\n  name: n\n  prompt: body\n");
  });
});

describe("removeSystemPrompt", () => {
  it("strips the block from the middle and preserves surrounding fields", () => {
    const existing =
      "identity: alice\nsystem_prompt:\n  name: gone\n  prompt: gone\nmode: open\n";
    const out = removeSystemPrompt(existing);
    assert.equal(out.content, "identity: alice\nmode: open\n");
    assert.equal(out.noop, false);
  });

  it("strips the block from EOF and trims trailing gap", () => {
    const existing = "identity: alice\nsystem_prompt:\n  name: gone\n  prompt: gone\n";
    const out = removeSystemPrompt(existing);
    assert.equal(out.content, "identity: alice\n");
  });

  it("no-ops when no block exists", () => {
    const existing = "identity: alice\n";
    const out = removeSystemPrompt(existing);
    assert.equal(out.noop, true);
    assert.equal(out.content, existing);
  });

  it("leaves an empty file when the block was the only content", () => {
    const existing = "system_prompt:\n  name: gone\n  prompt: gone\n";
    const out = removeSystemPrompt(existing);
    assert.equal(out.content.trim(), "");
  });

  it("collapses double-blank gap from middle removal", () => {
    const existing =
      "identity: alice\n\nsystem_prompt:\n  name: gone\n  prompt: gone\n\nmode: open\n";
    const out = removeSystemPrompt(existing);
    assert.equal(out.content, "identity: alice\n\nmode: open\n");
  });
});

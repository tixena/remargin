import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { buildClaudePrompt } from "./RemarginBackend.ts";

describe("buildClaudePrompt", () => {
  it("emits the inline-prompt argv when no slash command is set", () => {
    const out = buildClaudePrompt("be brief", ["a.md", "b.md"]);
    assert.equal(out, "be brief\n\nFiles:\na.md\nb.md");
  });

  it("omits the files block when the file list is empty", () => {
    assert.equal(buildClaudePrompt("be brief", []), "be brief");
  });

  it("emits the slash-command argv when useSlashCommand is set", () => {
    const out = buildClaudePrompt("", [], {
      command: "remargin:process-sandbox-group",
      arg: "writing-prompt",
    });
    assert.equal(out, "/remargin:process-sandbox-group writing-prompt");
  });

  it("omits the trailing space when the slash command has no arg", () => {
    const out = buildClaudePrompt("", [], { command: "remargin:process-file" });
    assert.equal(out, "/remargin:process-file");
  });

  it("ignores prompt and files when slash command is set", () => {
    const out = buildClaudePrompt("ignored", ["x.md"], {
      command: "remargin:process-file",
      arg: "y.md",
    });
    assert.equal(out, "/remargin:process-file y.md");
  });
});

/** Tests for composing the Submit flow's shell line and prompt files. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import {
  buildSubmitShellLine,
  composeInlinePrompt,
  defaultRunner,
  promptFileSlug,
  shellQuote,
} from "./submitCommand.ts";

describe("shellQuote", () => {
  it("wraps a plain string in single quotes", () => {
    assert.equal(shellQuote("hello"), "'hello'");
  });

  it("escapes embedded single quotes", () => {
    assert.equal(shellQuote("it's"), "'it'\\''s'");
  });

  it("leaves shell-special characters inert inside the quotes", () => {
    assert.equal(shellQuote('a "b" $c `d`'), "'a \"b\" $c `d`'");
  });
});

describe("defaultRunner", () => {
  it("bakes claude flags and the remargin mcp-config", () => {
    const out = defaultRunner("claude", "remargin");
    assert.equal(
      out,
      `'claude' -p --permission-mode auto --allowedTools "mcp__remargin__*" --mcp-config '{"mcpServers":{"remargin":{"command":"remargin","args":["mcp","run"]}}}'`
    );
  });

  it("uses resolved binary paths verbatim", () => {
    const out = defaultRunner("/opt/bin/claude", "/opt/bin/remargin");
    assert.ok(out.startsWith("'/opt/bin/claude' -p "), out);
    assert.ok(out.includes('"command":"/opt/bin/remargin"'), out);
  });

  it("falls back to bare names for blank settings", () => {
    const out = defaultRunner("  ", "");
    assert.ok(out.startsWith("'claude' "), out);
    assert.ok(out.includes('"command":"remargin"'), out);
  });
});

describe("composeInlinePrompt", () => {
  it("appends the file list and no marker-cleanup instruction", () => {
    const out = composeInlinePrompt("Review everything.", ["a.md", "b.md"]);
    assert.equal(out, "Review everything.\n\nFiles:\na.md\nb.md\n");
  });

  it("omits the Files block when the list is empty", () => {
    const out = composeInlinePrompt("Review.", []);
    assert.equal(out, "Review.\n");
  });
});

describe("buildSubmitShellLine", () => {
  it("emits one cat-pipe per entry chained with ';'", () => {
    const line = buildSubmitShellLine([
      { promptFile: "/tmp/x/a.md", runner: "goose run -i -" },
      { promptFile: "/tmp/x/b.md", runner: "'claude' -p" },
    ]);
    assert.equal(line, "cat '/tmp/x/a.md' | goose run -i -; cat '/tmp/x/b.md' | 'claude' -p");
  });

  it("quotes prompt-file paths containing spaces and quotes", () => {
    const line = buildSubmitShellLine([{ promptFile: "/tmp/it's here/p.md", runner: "r" }]);
    assert.equal(line, "cat '/tmp/it'\\''s here/p.md' | r");
  });

  it("appends the on-success sandbox removal under the submitter identity", () => {
    const line = buildSubmitShellLine(
      [{ promptFile: "/tmp/x/a.md", runner: "goose run -i -", files: ["docs/a.md", "b's.md"] }],
      { remarginPath: "/opt/bin/remargin", identityArgs: ["--config", "/home/me/.remargin.yaml"] }
    );
    assert.equal(
      line,
      "cat '/tmp/x/a.md' | goose run -i - && " +
        "'/opt/bin/remargin' '--config' '/home/me/.remargin.yaml' sandbox remove 'docs/a.md' 'b'\\''s.md'"
    );
  });

  it("skips the removal for entries without staged files", () => {
    const line = buildSubmitShellLine(
      [
        { promptFile: "/tmp/x/a.md", runner: "r1", files: [] },
        { promptFile: "/tmp/x/b.md", runner: "r2", files: ["f.md"] },
      ],
      { remarginPath: "remargin", identityArgs: [] }
    );
    assert.equal(
      line,
      "cat '/tmp/x/a.md' | r1; cat '/tmp/x/b.md' | r2 && 'remargin' sandbox remove 'f.md'"
    );
  });
});

describe("promptFileSlug", () => {
  it("lowercases and hyphenates non-alphanumerics", () => {
    assert.equal(promptFileSlug("SWE Reviewer"), "swe-reviewer");
  });

  it("trims leading/trailing hyphens", () => {
    assert.equal(promptFileSlug("  ~weird name!  "), "weird-name");
  });

  it("falls back to 'default' for empty input", () => {
    assert.equal(promptFileSlug("   "), "default");
  });
});

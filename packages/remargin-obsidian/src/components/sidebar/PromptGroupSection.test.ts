/** Static-markup tests for one prompt group of the Sandbox section. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { ResolvedSystemPrompt } from "../../backend/types.ts";
import type { PromptGroup } from "./buildPromptGroups.ts";
import type { InlinePromptEditorSaveArgs } from "./InlinePromptEditor.tsx";
import { PromptGroupSection, type PromptGroupSectionProps } from "./SandboxSection.tsx";

// SSR-only: first-render defaults are headerOpen=true and editing=false, and nothing changes them.

function explicit(overrides: Partial<PromptGroup> = {}): PromptGroup {
  const prompt: ResolvedSystemPrompt = {
    is_default: false,
    name: "Draft prompt",
    prompt: "## Draft\nbody",
    source: "notes/foo/.remargin.yaml",
  };
  return {
    source: "notes/foo/.remargin.yaml",
    name: "Draft prompt",
    scope: "notes/foo",
    prompt,
    files: ["notes/foo/a.md", "notes/foo/b.md"],
    staged: ["notes/foo/a.md"],
    unstaged: ["notes/foo/b.md"],
    isDefault: false,
    ...overrides,
  };
}

function defaultGroup(overrides: Partial<PromptGroup> = {}): PromptGroup {
  const prompt: ResolvedSystemPrompt = {
    is_default: true,
    name: "default",
    prompt: "",
    source: null,
  };
  return {
    source: null,
    name: "default",
    scope: "(vault)",
    prompt,
    files: ["notes/orphan.md"],
    staged: [],
    unstaged: ["notes/orphan.md"],
    isDefault: true,
    ...overrides,
  };
}

function errorGroup(overrides: Partial<PromptGroup> = {}): PromptGroup {
  const prompt: ResolvedSystemPrompt = {
    is_default: true,
    name: "(error)",
    prompt: "",
    source: null,
  };
  return {
    source: null,
    name: "(error)",
    scope: "resolve failed",
    prompt,
    files: ["notes/broken.md"],
    staged: [],
    unstaged: ["notes/broken.md"],
    isDefault: false,
    hasError: true,
    errorMessage: "walk failed: parent .remargin.yaml is unreadable",
    ...overrides,
  };
}

const noop = (): void => {
  /* test-only no-op */
};
const noopAsyncSave = async (_args: InlinePromptEditorSaveArgs): Promise<void> => {
  /* test-only no-op */
};
const noopAsyncDelete = async (_source: string): Promise<void> => {
  /* test-only no-op */
};

function baseProps(group: PromptGroup): PromptGroupSectionProps {
  return {
    group,
    viewMode: "flat",
    selected: new Set<string>(),
    stagedOpen: true,
    unstagedOpen: true,
    onToggleStagedOpen: noop,
    onToggleUnstagedOpen: noop,
    onToggleSelected: noop,
    onStageBulk: noop,
    onUnstageBulk: noop,
    onSelectAll: noop,
    onRemoveFile: noop,
    onOpenFile: noop,
  };
}

function render(group: PromptGroup, overrides: Partial<PromptGroupSectionProps> = {}): string {
  return renderToStaticMarkup(
    createElement(PromptGroupSection, { ...baseProps(group), ...overrides })
  );
}

describe("PromptGroupSection — header chrome", () => {
  it("renders the prompt name and scope", () => {
    const html = render(explicit());
    assert.ok(html.includes(">Draft prompt<"), `expected prompt name, got: ${html.slice(0, 300)}`);
    assert.ok(
      html.includes("./notes/foo/"),
      `expected vault-relative scope label, got: ${html.slice(0, 300)}`
    );
  });

  it("renders the file count badge", () => {
    const html = render(explicit({ files: ["a.md", "b.md", "c.md"] }));
    assert.ok(html.includes(">3<"), `expected file count of 3, got: ${html}`);
  });

  it("uses CircleDashed icon for the Default group (vs Sparkles for explicit)", () => {
    const explicitHtml = render(explicit());
    const defaultHtml = render(defaultGroup());
    assert.ok(explicitHtml.includes("lucide-sparkles"), "explicit group must use sparkles icon");
    assert.ok(
      defaultHtml.includes("lucide-circle-dashed"),
      "default group must use circle-dashed icon"
    );
  });

  it("shows the chevron-down icon when the header is open (initial render)", () => {
    const html = render(explicit());
    assert.ok(html.includes("lucide-chevron-down"), `expected open chevron, got: ${html}`);
  });
});

describe("PromptGroupSection — no per-group status chrome", () => {
  it("renders no submit status icons (fire-and-forget submit)", () => {
    const html = render(explicit());
    assert.ok(!html.includes("lucide-loader-circle"), "no spinner");
    assert.ok(!html.includes('aria-label="Submitted"'), "no success check");
    assert.ok(!html.includes('aria-label="Submit failed"'), "no error triangle");
  });
});

describe("PromptGroupSection — edit / configure affordances", () => {
  it("renders the gear button with 'Edit prompt' title for explicit groups", () => {
    const html = render(explicit(), { onSavePrompt: noopAsyncSave });
    assert.ok(
      html.includes('title="Edit prompt"'),
      `expected Edit prompt gear title, got: ${html}`
    );
    // Obsidian paints the gear glyph at runtime, so the title attribute is the testable contract.
  });

  it("renders the '+ Configure' affordance ONLY on the Default group", () => {
    const defaultHtml = render(defaultGroup(), { onSavePrompt: noopAsyncSave });
    const explicitHtml = render(explicit(), { onSavePrompt: noopAsyncSave });
    assert.ok(
      defaultHtml.includes("+ Configure"),
      `expected + Configure on default group, got: ${defaultHtml}`
    );
    assert.ok(
      !explicitHtml.includes("+ Configure"),
      `explicit group must NOT render + Configure, got: ${explicitHtml}`
    );
  });

  it("does NOT render gear or +Configure on the error group (hasError)", () => {
    const html = render(errorGroup(), { onSavePrompt: noopAsyncSave });
    assert.ok(
      !html.includes('title="Edit prompt"'),
      `error group must not render edit gear, got: ${html}`
    );
    assert.ok(
      !html.includes("+ Configure"),
      `error group must not render + Configure, got: ${html}`
    );
  });

  it("surfaces the error message via the header title on the error group", () => {
    const html = render(errorGroup());
    assert.ok(
      html.includes("walk failed: parent .remargin.yaml is unreadable"),
      `expected error tooltip text, got: ${html}`
    );
  });

  it("does NOT mount the InlinePromptEditor on initial render (editing=false)", () => {
    const html = render(explicit(), {
      onSavePrompt: noopAsyncSave,
      onDeletePrompt: noopAsyncDelete,
    });
    assert.ok(
      !html.includes('data-testid="inline-prompt-editor"'),
      `editor must NOT appear before the gear is clicked, got: ${html}`
    );
  });
});

describe("PromptGroupSection — sub-section delegation", () => {
  it("renders both Staged and Unstaged sub-headers when headerOpen", () => {
    const html = render(explicit());
    assert.ok(html.includes("Staged"), `expected Staged header, got: ${html.slice(0, 500)}`);
    assert.ok(html.includes("Unstaged"), `expected Unstaged header, got: ${html.slice(0, 500)}`);
  });

  it("renders staged file rows when stagedOpen=true", () => {
    const html = render(explicit({ staged: ["notes/foo/a.md"], unstaged: [] }), {
      stagedOpen: true,
      unstagedOpen: false,
    });
    assert.ok(
      html.includes("a.md"),
      `expected staged file 'a.md' to render, got: ${html.slice(0, 500)}`
    );
  });

  it("renders unstaged file rows when unstagedOpen=true", () => {
    const html = render(explicit({ staged: [], unstaged: ["notes/foo/b.md"] }), {
      stagedOpen: false,
      unstagedOpen: true,
    });
    assert.ok(
      html.includes("b.md"),
      `expected unstaged file 'b.md' to render, got: ${html.slice(0, 500)}`
    );
  });
});

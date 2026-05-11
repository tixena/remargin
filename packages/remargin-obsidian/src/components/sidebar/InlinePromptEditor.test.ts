import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  InlinePromptEditor,
  type InlinePromptEditorProps,
  type InlinePromptEditorSaveArgs,
} from "./InlinePromptEditor.tsx";

// renderToStaticMarkup skips useEffect, so the CM6 mount never runs in
// these tests — the @codemirror import stays cold.

const noopSave = async (_args: InlinePromptEditorSaveArgs): Promise<void> => {
  /* test-only no-op */
};
const noopDelete = async (_source: string): Promise<void> => {
  /* test-only no-op */
};
const noopCancel = (): void => {
  /* test-only no-op */
};

function baseProps(overrides: Partial<InlinePromptEditorProps>): InlinePromptEditorProps {
  return {
    source: null,
    folder: "notes/foo",
    initialName: "",
    initialBody: "",
    onSave: noopSave,
    onCancel: noopCancel,
    ...overrides,
  };
}

function render(overrides: Partial<InlinePromptEditorProps>): string {
  return renderToStaticMarkup(createElement(InlinePromptEditor, baseProps(overrides)));
}

describe("InlinePromptEditor — create vs edit chrome", () => {
  it("renders 'Create prompt' header and 'Create' save label when source is null", () => {
    const html = render({ source: null });
    assert.ok(html.includes("Create prompt"), `expected create header, got: ${html.slice(0, 300)}`);
    assert.ok(
      />Create<\/button>|>Create<\/span>|>Create</.test(html),
      `expected Create button label, got: ${html}`
    );
    assert.ok(!html.includes("Editing prompt"), "edit header must not appear in create mode");
  });

  it("renders 'Editing prompt' header and 'Save' save label when source is set", () => {
    const html = render({ source: "notes/foo/.remargin.yaml", initialName: "Drafting" });
    assert.ok(html.includes("Editing prompt"), `expected edit header, got: ${html.slice(0, 300)}`);
    assert.ok(/>Save</.test(html), `expected Save button label, got: ${html}`);
    assert.ok(!html.includes("Create prompt"), "create header must not appear in edit mode");
  });

  it("renders a stable data-testid wrapper for downstream UI tests", () => {
    const html = render({});
    assert.ok(
      html.includes('data-testid="inline-prompt-editor"'),
      `expected data-testid hook, got: ${html.slice(0, 200)}`
    );
  });

  it("computes the target file from folder when creating", () => {
    const html = render({ source: null, folder: "notes/foo" });
    assert.ok(
      html.includes("notes/foo/.remargin.yaml"),
      `expected derived target path, got: ${html}`
    );
  });

  it("strips a trailing slash from folder when deriving the target path", () => {
    const html = render({ source: null, folder: "notes/foo/" });
    assert.ok(
      html.includes("notes/foo/.remargin.yaml"),
      `expected normalised target path, got: ${html}`
    );
    assert.ok(
      !html.includes("notes/foo//.remargin.yaml"),
      "trailing-slash folder must collapse the double separator"
    );
  });

  it("uses the source path verbatim as the target label when editing", () => {
    const html = render({
      source: "notes/bar/.remargin.yaml",
      folder: "notes/bar",
      initialName: "Bar prompt",
    });
    assert.ok(
      html.includes("notes/bar/.remargin.yaml"),
      `expected source path label, got: ${html}`
    );
  });
});

describe("InlinePromptEditor — name input wiring", () => {
  it("seeds the name input from initialName", () => {
    const html = render({ initialName: "Drafting prompt" });
    assert.ok(
      /<input[^>]*value="Drafting prompt"/.test(html),
      `expected name input seeded, got: ${html}`
    );
  });

  it("falls back to empty value when initialName is blank", () => {
    const html = render({ initialName: "" });
    assert.ok(/<input[^>]*value=""/.test(html), `expected empty name input, got: ${html}`);
  });
});

describe("InlinePromptEditor — Delete affordance", () => {
  it("renders Delete when editing AND onDelete is provided", () => {
    const html = render({
      source: "notes/foo/.remargin.yaml",
      initialName: "Foo",
      onDelete: noopDelete,
    });
    assert.ok(html.includes(">Delete<"), `expected Delete label, got: ${html}`);
    assert.ok(html.includes("lucide-trash-2"), `expected trash icon, got: ${html}`);
  });

  it("does NOT render Delete when creating, even if onDelete is supplied", () => {
    // Default group flow: source=null AND onDelete set must not show Delete
    // — only existing prompts can be deleted (matches the source check
    // inside the editor).
    const html = render({ source: null, onDelete: noopDelete });
    assert.ok(!html.includes(">Delete<"), `Delete must not show in create mode, got: ${html}`);
  });

  it("does NOT render Delete when editing but onDelete is omitted", () => {
    const html = render({
      source: "notes/foo/.remargin.yaml",
      initialName: "Foo",
    });
    assert.ok(!html.includes(">Delete<"), `Delete must be omitted without handler, got: ${html}`);
  });
});

describe("InlinePromptEditor — strict-mode disabled state", () => {
  // The Save/Create button is the only one styled with `bg-accent text-white`
  // (the ghost Cancel button uses `hover:bg-accent` which the unprefixed
  // class match below ignores).
  const SAVE_BUTTON_RE = /<button[^>]*\bbg-accent text-white\b[^>]*>[\s\S]*?<\/button>/;

  it("disables the Save button when saveDisabledReason is set", () => {
    const html = render({ saveDisabledReason: "strict mode requires a signing key" });
    const saveButtonMatch = html.match(SAVE_BUTTON_RE);
    assert.ok(saveButtonMatch, `expected to find the save button, got: ${html}`);
    assert.ok(
      /\sdisabled(?:=""|>|\s)/.test(saveButtonMatch[0]),
      `expected save button to carry disabled, got: ${saveButtonMatch[0]}`
    );
  });

  it("surfaces saveDisabledReason via the title attribute for tooltip", () => {
    const html = render({ saveDisabledReason: "strict mode requires a signing key" });
    assert.ok(
      html.includes('title="strict mode requires a signing key"'),
      `expected disabled-reason tooltip, got: ${html}`
    );
  });

  it("leaves Save enabled when saveDisabledReason is undefined", () => {
    const html = render({});
    const saveButtonMatch = html.match(SAVE_BUTTON_RE);
    assert.ok(saveButtonMatch, `expected to find the save button, got: ${html}`);
    assert.ok(
      !/\sdisabled(?:=""|>|\s)/.test(saveButtonMatch[0]),
      `Save must not be disabled by default, got: ${saveButtonMatch[0]}`
    );
  });
});

describe("InlinePromptEditor — Cancel + close affordances", () => {
  it("renders an explicit Cancel button", () => {
    const html = render({});
    assert.ok(/>Cancel</.test(html), `expected Cancel button, got: ${html}`);
  });

  it("renders the header X close affordance with title='Cancel'", () => {
    const html = render({});
    // The header X button uses `title="Cancel"` and lucide-x icon.
    assert.ok(
      /title="Cancel"[^>]*>\s*<svg[^>]*lucide-x/.test(html) || /title="Cancel"/.test(html),
      `expected close X affordance, got: ${html}`
    );
  });
});

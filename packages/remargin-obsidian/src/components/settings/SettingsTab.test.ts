/** Static-render tests for the editor-widgets toggle in the settings tab. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { RemarginBackend } from "../../backend/index.ts";
import { BackendContext } from "../../hooks/useBackend.ts";
import { DEFAULT_SETTINGS, type RemarginSettings } from "../../types.ts";
import { SettingsTab } from "./SettingsTab.tsx";

// `resolveMode` runs in a useEffect that static render skips; a never-resolving promise is enough.
const backendStub = {
  resolveMode: (): Promise<{ mode: string | undefined }> =>
    new Promise(() => {
      /* never resolves — useEffect is skipped under static render */
    }),
} as unknown as RemarginBackend;

const noopSave = (_: RemarginSettings) => {
  /* test-only no-op save handler */
};

function render(settings: RemarginSettings, onSave: (s: RemarginSettings) => void): string {
  return renderToStaticMarkup(
    createElement(
      BackendContext.Provider,
      { value: backendStub },
      createElement(SettingsTab, {
        settings,
        onSave,
        onCheckUpdates: async () => settings,
      })
    )
  );
}

describe("SettingsTab — editor widgets toggle", () => {
  it("renders the editor widgets label and description copy verbatim", () => {
    const html = render({ ...DEFAULT_SETTINGS }, noopSave);
    assert.ok(html.includes("Editor widgets"), `expected 'Editor widgets' label, got: ${html}`);
    assert.ok(
      html.includes(
        "Pretty-print remargin comment blocks in Live Preview and reading mode (read-only)."
      ),
      `expected description text, got: ${html}`
    );
  });

  // One Radix toggle button: `aria-pressed="false"`, `data-state="off"`, labelled "Disabled".
  it("toggle reflects editorWidgets=false (the default)", () => {
    const html = render({ ...DEFAULT_SETTINGS, editorWidgets: false }, noopSave);
    const widgetsBlock = sliceBlock(html, "Editor widgets", "Check for updates");
    assert.match(
      widgetsBlock,
      /<button[^>]*aria-pressed="false"[^>]*data-state="off"[^>]*>\s*Disabled\s*<\/button>/,
      `expected unpressed Disabled button when editorWidgets is false, got: ${widgetsBlock}`
    );
  });

  it("toggle reflects editorWidgets=true", () => {
    const html = render({ ...DEFAULT_SETTINGS, editorWidgets: true }, noopSave);
    const widgetsBlock = sliceBlock(html, "Editor widgets", "Check for updates");
    assert.match(
      widgetsBlock,
      /<button[^>]*aria-pressed="true"[^>]*data-state="on"[^>]*>\s*Enabled\s*<\/button>/,
      `expected pressed Enabled button when editorWidgets is true, got: ${widgetsBlock}`
    );
  });
});

/**
 * Carve out the block of markup between two anchor strings — used so
 * each assertion only inspects the editor-widgets row, not the rest
 * of the SettingsTab (which has its own On/Off toggles).
 */
function sliceBlock(html: string, startNeedle: string, endNeedle: string): string {
  const start = html.indexOf(startNeedle);
  const end = html.indexOf(endNeedle, start + startNeedle.length);
  if (start < 0) return "";
  return html.slice(start, end < 0 ? undefined : end);
}

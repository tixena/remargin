import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { RemarginBackend } from "../../backend/index.ts";
import { BackendContext } from "../../hooks/useBackend.ts";
import { DEFAULT_SETTINGS, type RemarginSettings } from "../../types.ts";
import { SettingsTab } from "./SettingsTab.tsx";

// Minimal backend stub — the tab calls `resolveMode` inside a useEffect,
// which static-render skips, so a never-resolving promise is enough.
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

describe("SettingsTab — editor widgets toggle (T36 AC #13)", () => {
  // Verifies the copy required by the spec is on screen.
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

  // Verifies the toggle reflects `settings.editorWidgets === false`. The
  // ToggleGroup renders `data-state="on"` on whichever option is active.
  // Default settings have editorWidgets=false, so 'off' must be active.
  it("toggle reflects editorWidgets=false (the default)", () => {
    const html = render({ ...DEFAULT_SETTINGS, editorWidgets: false }, noopSave);
    // The 'On' button is rendered first, 'Off' second. We need to find
    // the segment between the editor widgets section and the next
    // separator and check which has data-state=on. The simplest
    // assertion: the rendered DOM contains an Off button with
    // data-state="on" inside the editor-widgets row.
    const widgetsBlock = sliceBlock(html, "Editor widgets", "Check for updates");
    assert.match(
      widgetsBlock,
      /<button[^>]*data-state="on"[^>]*>\s*Off\s*<\/button>/,
      `expected Off to be active when editorWidgets is false, got: ${widgetsBlock}`
    );
  });

  // And the inverse: with editorWidgets=true, the 'On' button is the
  // active toggle.
  it("toggle reflects editorWidgets=true", () => {
    const html = render({ ...DEFAULT_SETTINGS, editorWidgets: true }, noopSave);
    const widgetsBlock = sliceBlock(html, "Editor widgets", "Check for updates");
    assert.match(
      widgetsBlock,
      /<button[^>]*data-state="on"[^>]*>\s*On\s*<\/button>/,
      `expected On to be active when editorWidgets is true, got: ${widgetsBlock}`
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

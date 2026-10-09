/**
 * Test-time stub for the `obsidian` module. The real npm package ships
 * type declarations only (`main: ""`), so any test that walks through
 * code importing from "obsidian" would fail at runtime without a stub.
 *
 * The exports here cover the surface area component code in this
 * package touches. Each is a minimal no-op — tests that actually
 * exercise behaviour (e.g. setIcon side-effects) override the relevant
 * export with a spy before the component renders.
 */

const noop = () => {
  /* obsidian-module no-op stub */
};

// Called inside `useEffect`, which react-dom/server skips; a real function for client renders.
export const setIcon = noop;

export const MarkdownRenderer = {
  render: async () => {
    /* obsidian-module no-op stub */
  },
};

// `requestUrl` is referenced from the plugin's release-fetcher. Tests
// inject their own fetcher, so this is just a placeholder so static
// imports resolve.
export const requestUrl = async () => ({ status: 0, text: "", json: {} });

/**
 * Stub of the plugin base class, the one `plugin.test.ts` constructs: it stashes `app` and
 * `manifest` and exposes enough register-style methods to swallow onload's calls.
 */
export class Plugin {
  constructor(app, manifest) {
    this.app = app;
    this.manifest = manifest;
  }
  // No-op registration helpers. Real Obsidian wires these into its
  // workspace lifecycle; tests only care that they are callable.
  addCommand() {
    /* obsidian-module no-op stub */
  }
  addRibbonIcon() {
    return { addEventListener: noop };
  }
  addSettingTab() {
    /* obsidian-module no-op stub */
  }
  registerView() {
    /* obsidian-module no-op stub */
  }
  registerEvent() {
    /* obsidian-module no-op stub */
  }
  registerEditorExtension() {
    /* obsidian-module no-op stub */
  }
  registerMarkdownPostProcessor() {
    /* obsidian-module no-op stub */
  }
  // Settings persistence stubs — the real plugin reads via
  // `loadData` / `saveData`. Tests inject their own backing map by
  // overriding these on the instance.
  async loadData() {
    return null;
  }
  async saveData() {
    /* obsidian-module no-op stub */
  }
}
/** Stub base class for the sidebar view; extended, never rendered. */
export class ItemView {
  constructor() {
    /* obsidian-module no-op stub */
  }
}
/** Placeholder for the markdown editor view, passed around as a type token. */
export class MarkdownView {}
/**
 * Stub of the render child the reading-mode widget extends. Like the real one it stashes its
 * host element on `containerEl`, so subclasses can mount into it during `onload()`.
 */
export class MarkdownRenderChild {
  constructor(containerEl) {
    this.containerEl = containerEl;
  }
}
/** Stub base class for the settings tab; extended, never displayed. */
export class PluginSettingTab {
  constructor() {
    /* obsidian-module no-op stub */
  }
}
/** Stub of the toast notice; constructing one does nothing. */
export class Notice {
  constructor() {
    /* obsidian-module no-op stub */
  }
}
/** Stub of the settings-row builder; constructing one does nothing. */
export class Setting {
  constructor() {
    /* obsidian-module no-op stub */
  }
}

// A placeholder symbol: tests pass their own `state.field` keyed on it.
export const editorInfoField = {};

// An identity-stable placeholder: tests key their own `state.field` on this sentinel.
export const editorLivePreviewField = {};

// `setting`/`workspace`-shaped helpers some components reference.
// Empty stubs are safe because no static-render path consumes them.
export const moment = (input) => ({ valueOf: () => Date.now(), input });

// `normalizePath` converts OS path separators to forward slashes, strips a
// leading `./`, and collapses repeated slashes — matching real Obsidian
// behaviour closely enough for unit tests.
export function normalizePath(path) {
  return path.replaceAll("\\", "/").replace(/^\.\//, "").replace(/\/+/g, "/");
}

/** Placeholder for a vault file; component code names it in `type` imports and type checks. */
export class TFile {}
/** Placeholder for a vault folder. */
export class TFolder {}
/** Placeholder for the common base of vault files and folders. */
export class TAbstractFile {}
/** Placeholder for a workspace leaf, one pane of the app. */
export class WorkspaceLeaf {}
/** Placeholder for the Obsidian app object; tests pass their own mock in its place. */
export class App {}
/** Placeholder for the workspace object. */
export class Workspace {}

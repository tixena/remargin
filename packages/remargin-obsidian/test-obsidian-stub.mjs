/**
 * Test-time stub for the `obsidian` module. The real npm package ships
 * type declarations only (`main: ""`), so any test that walks through
 * code importing from "obsidian" would fail at runtime without a stub.
 *
 * The exports here cover the surface area component code in this
 * package touches today. Each is a minimal no-op — tests that actually
 * exercise behaviour (e.g. setIcon side-effects) override the relevant
 * export with a spy before the component renders.
 *
 * Authoritative for: T36 (rem-fyj8.1) — see the ticket's "Mocks
 * authorized" table. Add new exports here as new components import
 * additional Obsidian APIs at test scope.
 */

const noop = () => {
  /* obsidian-module no-op stub */
};

// Lightweight `setIcon`: components call this inside `useEffect`, which
// react-dom/server skips, so the stub almost never fires in static
// markup tests. Kept as a real function so client-render tests don't
// throw if they exercise it.
export const setIcon = noop;

// `MarkdownRenderer.render` is awaited inside MarkdownContent's
// useEffect — also unreachable in static markup tests, but exposed so
// the namespace import resolves cleanly.
export const MarkdownRenderer = {
  render: async () => {
    /* obsidian-module no-op stub */
  },
};

// `requestUrl` is referenced from the plugin's release-fetcher. Tests
// inject their own fetcher, so this is just a placeholder so static
// imports resolve.
export const requestUrl = async () => ({ status: 0, text: "", json: {} });

// Plugin-side base classes. Most are reference-only at runtime
// (declared inside `class Foo extends X`), but `Plugin` IS constructed
// by `plugin.test.ts` so the stub mirrors the real signature: stash
// `app` + `manifest`, expose enough register-style methods to swallow
// onload's calls without crashing. Tests pass a minimal mock `app`
// shaped just enough to satisfy the paths they exercise.
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
export class ItemView {
  constructor() {
    /* obsidian-module no-op stub */
  }
}
export class MarkdownView {}
export class MarkdownRenderChild {
  constructor() {
    /* obsidian-module no-op stub */
  }
}
export class PluginSettingTab {
  constructor() {
    /* obsidian-module no-op stub */
  }
}
export class Notice {
  constructor() {
    /* obsidian-module no-op stub */
  }
}
export class Setting {
  constructor() {
    /* obsidian-module no-op stub */
  }
}

// `setting`/`workspace`-shaped helpers some components reference.
// Empty stubs are safe because no static-render path consumes them.
export const moment = (input) => ({ valueOf: () => Date.now(), input });

// Vault types referenced via `type` imports in component code. Tests
// never construct these, but type-only imports are erased before
// runtime so the named exports just need to exist.
export class TFile {}
export class TFolder {}
export class TAbstractFile {}
export class WorkspaceLeaf {}
export class App {}
export class Workspace {}

import {
  ItemView,
  MarkdownView,
  Notice,
  Plugin,
  PluginSettingTab,
  requestUrl,
  type WorkspaceLeaf,
} from "obsidian";
import { createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { RemarginBackend } from "./backend";
import { RemarginSidebar } from "./components/RemarginSidebar";
import { SettingsTab } from "./components/settings/SettingsTab";
import { BackendContext } from "./hooks/useBackend";
import { PluginContext } from "./hooks/usePlugin";
import { PortalContainerContext } from "./hooks/usePortalContainer";
import { detectNewUpdates, type ReleasesFetcher, type UpdateComponent } from "./lib/githubReleases";
import { snapAfterCommentBlock } from "./lib/line-snap";
// import { commentWidgetPlugin } from "./editor/commentWidget";
// import { remarginPostProcessor } from "./editor/readingModeProcessor";
import { DEFAULT_SETTINGS, type RemarginSettings } from "./types";
import "./styles/globals.css";

export const VIEW_TYPE_REMARGIN = "remargin-sidebar";

/** How long a startup update Notice stays on screen. */
const UPDATE_NOTICE_MS = 8000;

/**
 * Human-readable name for each component shown in the Notice. Kept next
 * to the Notice call site so future copy changes live in one place.
 */
const COMPONENT_LABELS: Record<UpdateComponent, string> = {
  plugin: "plugin",
  cli: "CLI",
};

/**
 * Adapter that turns Obsidian's CORS-free `requestUrl` into the
 * `ReleasesFetcher` shape the update-check pipeline consumes. Extracted
 * so tests can inject a pure in-memory stub without touching Obsidian.
 */
const obsidianReleasesFetcher: ReleasesFetcher = async (url) => {
  try {
    const response = await requestUrl({
      url,
      method: "GET",
      headers: {
        Accept: "application/vnd.github+json",
        "User-Agent": "remargin-obsidian",
      },
      throw: false,
    });
    return {
      ok: response.status >= 200 && response.status < 300,
      status: response.status,
      body: response.text ?? "",
    };
  } catch (err) {
    return {
      ok: false,
      status: 0,
      body: err instanceof Error ? err.message : "requestUrl failed",
    };
  }
};

class RemarginView extends ItemView {
  private root: Root | null = null;

  constructor(
    leaf: WorkspaceLeaf,
    private plugin: RemarginPlugin
  ) {
    super(leaf);
  }

  async onOpen() {
    const container = this.containerEl.children[1] as HTMLElement;
    container.empty();
    container.addClass("remargin-container");
    this.root = createRoot(container);
    this.root.render(
      createElement(
        PluginContext.Provider,
        { value: this.plugin },
        createElement(
          BackendContext.Provider,
          { value: this.plugin.backend },
          createElement(
            PortalContainerContext.Provider,
            { value: container },
            createElement(RemarginSidebar, { plugin: this.plugin })
          )
        )
      )
    );
  }

  async onClose() {
    this.root?.unmount();
  }

  getViewType(): string {
    return VIEW_TYPE_REMARGIN;
  }

  getDisplayText(): string {
    return "Remargin";
  }

  getIcon(): string {
    return "message-square";
  }
}

class RemarginSettingTab extends PluginSettingTab {
  private root: Root | null = null;

  constructor(private plugin: RemarginPlugin) {
    super(plugin.app, plugin);
  }

  display() {
    this.containerEl.empty();
    const mount = this.containerEl.createDiv({ cls: "remargin-container" });
    this.root = createRoot(mount);
    this.root.render(
      createElement(
        BackendContext.Provider,
        { value: this.plugin.backend },
        createElement(
          PortalContainerContext.Provider,
          { value: mount },
          createElement(SettingsTab, {
            settings: this.plugin.settings,
            onSave: (s: RemarginSettings) => this.plugin.saveSettings(s),
          })
        )
      )
    );
  }

  hide() {
    this.root?.unmount();
    this.root = null;
  }
}

/** Payload the sidebar uses to open its inline comment composer. */
export interface ComposeRequest {
  file: string;
  afterLine: number;
}

export default class RemarginPlugin extends Plugin {
  settings: RemarginSettings = DEFAULT_SETTINGS;
  backend!: RemarginBackend;

  /**
   * Most recently focused markdown view. Used as the stable "active editor"
   * that survives clicks into the sidebar. `getActiveViewOfType(MarkdownView)`
   * flips to null the moment the sidebar leaf becomes active, so the `+`
   * button cannot rely on it. This cache is only *set* when the event fires
   * with a markdown view; it is never cleared just because focus moved.
   * It IS invalidated when the cached view's file is closed.
   */
  private lastMarkdownView: MarkdownView | null = null;

  /** Registered by RemarginSidebar on mount; called by `requestCompose`. */
  private composeHandler: ((request: ComposeRequest) => void) | null = null;

  /**
   * Compose request that arrived before the React sidebar registered its
   * handler (e.g. when the command is invoked while the sidebar is closed).
   * Drained on the next `setComposeHandler` call.
   */
  private pendingCompose: ComposeRequest | null = null;

  async onload() {
    await this.loadSettings();

    const adapter = this.app.vault.adapter as unknown as { basePath?: string };
    const vaultPath = adapter.basePath ?? "";
    this.backend = new RemarginBackend(this.settings, vaultPath);

    this.addSettingTab(new RemarginSettingTab(this));

    // this.registerEditorExtension([commentWidgetPlugin]);
    // this.registerMarkdownPostProcessor(remarginPostProcessor);

    this.registerView(VIEW_TYPE_REMARGIN, (leaf) => new RemarginView(leaf, this));

    // Seed the last-markdown-view cache from current workspace state.
    const initialView = this.app.workspace.getActiveViewOfType(MarkdownView);
    if (initialView) this.lastMarkdownView = initialView;

    // Keep the cache fresh. `active-leaf-change` and `file-open` both fire
    // when the user is actively editing a markdown file. We only *set* on a
    // non-null view -- never overwrite with null -- so the cached view
    // survives sidebar focus.
    this.registerEvent(
      this.app.workspace.on("active-leaf-change", () => {
        const view = this.app.workspace.getActiveViewOfType(MarkdownView);
        if (view) this.lastMarkdownView = view;
      })
    );
    this.registerEvent(
      this.app.workspace.on("file-open", () => {
        const view = this.app.workspace.getActiveViewOfType(MarkdownView);
        if (view) this.lastMarkdownView = view;
      })
    );
    // On layout change, invalidate the cache if the cached view's file is
    // gone (pane closed, file deleted, etc.).
    this.registerEvent(
      this.app.workspace.on("layout-change", () => {
        if (this.lastMarkdownView && !this.lastMarkdownView.file) {
          this.lastMarkdownView = null;
        }
      })
    );

    this.addCommand({
      id: "open-sidebar",
      name: "Open sidebar",
      callback: () => this.activateView(),
    });

    this.addCommand({
      id: "add-comment-at-cursor",
      name: "Add comment at cursor",
      callback: () => {
        void this.addComment();
      },
    });

    this.addCommand({
      id: "refresh",
      name: "Refresh comments",
      callback: () => {
        this.activateView();
      },
    });

    this.addCommand({
      id: "ack-comment",
      name: "Ack comment at cursor",
      editorCallback: async (editor) => {
        const file = this.app.workspace.getActiveFile();
        if (!file) return;
        const line = editor.getCursor().line + 1;
        const { parseRemarginBlocks } = await import("./parser");
        const text = editor.getValue();
        const blocks = parseRemarginBlocks(text);
        const block = blocks.find((b) => line >= b.startLine && line <= b.endLine);
        if (block?.comment.id) {
          await this.backend.ack(file.path, [block.comment.id]);
        }
      },
    });

    this.addRibbonIcon("message-square", "Open Remargin", () => {
      this.activateView();
    });

    this.app.workspace.onLayoutReady(() => {
      this.activateView();
    });

    // Kick off the version probe after the vault is ready. Runs entirely
    // in the background: any failure is folded into the check-failed
    // status and never bubbles up as an error Notice.
    void this.runUpdateCheck(false);
  }

  /**
   * Run the GitHub-releases update check and persist the result. Fires a
   * single unobtrusive Notice per component that transitioned to
   * `update-available` since the last cached snapshot.
   *
   * Honors the `checkForUpdates` settings toggle: when off, no fetcher
   * is invoked and no Notice fires. `force=true` bypasses both the cache
   * TTL and the toggle (used by the Settings "Check now" button — rem-9trw).
   */
  async runUpdateCheck(force: boolean): Promise<void> {
    if (!force && !this.settings.checkForUpdates) return;
    const installedPlugin = this.manifest.version;
    const before = this.settings.updateCheck;
    let after;
    try {
      after = await this.backend.checkForUpdates({
        force,
        installedPlugin,
        fetcher: obsidianReleasesFetcher,
        cache: before,
      });
    } catch {
      // The backend wrapper is supposed to swallow errors, but guard the
      // call site too so an unexpected bug can't crash `onload`.
      return;
    }
    // Short-circuit: cache was fresh and the wrapper returned it unchanged.
    if (after === before) return;

    // Persist through saveSettings so the backend's in-memory copy stays
    // in sync (it reads from the same settings object).
    await this.saveSettings({ ...this.settings, updateCheck: after });

    // Only fire Notices on the passive (non-forced) path so the Settings
    // "Check now" button — which renders its own inline status — does
    // not double-surface.
    if (force) return;
    const newlyAvailable = detectNewUpdates(before, after);
    for (const component of newlyAvailable) {
      const check = after[component];
      if (!check.latest) continue;
      new Notice(
        `Remargin ${COMPONENT_LABELS[component]} ${check.latest} available — open Settings → Updates`,
        UPDATE_NOTICE_MS
      );
    }
  }

  async loadSettings() {
    const saved = await this.loadData();
    if (saved) {
      // Migration: older plugin versions persisted a `remarginMode` field in
      // data.json that was never actually wired to the CLI. The vault-root
      // .remargin.yaml is now the single source of truth for mode, so drop
      // the ghost field on load (saveSettings will persist without it).
      if (saved && typeof saved === "object" && "remarginMode" in saved) {
        delete (saved as { remarginMode?: unknown }).remarginMode;
      }
      this.settings = Object.assign({}, DEFAULT_SETTINGS, saved);
      return;
    }
    // First run: ask the CLI where a human identity config lives by
    // walking up from the vault. If it finds one, use config mode with
    // the resolved path. Otherwise fall back to manual mode.
    this.settings = { ...DEFAULT_SETTINGS };
    try {
      const vaultPath = (this.app.vault.adapter as unknown as { basePath?: string }).basePath ?? "";
      const probe = new RemarginBackend(this.settings, vaultPath);
      const info = await probe.identity("human");
      if (info.found && info.path) {
        this.settings.identityMode = "config";
        this.settings.configFilePath = info.path;
      }
    } catch {
      // CLI not available or other error — keep manual defaults.
    }
  }

  async saveSettings(settings: RemarginSettings) {
    const previousSide = this.settings.sidebarSide;
    this.settings = settings;
    this.backend?.updateSettings(settings);
    await this.saveData(settings);

    if (previousSide !== settings.sidebarSide) {
      for (const leaf of this.app.workspace.getLeavesOfType(VIEW_TYPE_REMARGIN)) {
        leaf.detach();
      }
      await this.activateView();
    }
  }

  async activateView() {
    const leaves = this.app.workspace.getLeavesOfType(VIEW_TYPE_REMARGIN);
    if (leaves.length === 0) {
      const leaf =
        this.settings.sidebarSide === "right"
          ? this.app.workspace.getRightLeaf(false)
          : this.app.workspace.getLeftLeaf(false);
      if (leaf) {
        await leaf.setViewState({
          type: VIEW_TYPE_REMARGIN,
          active: true,
        });
      }
    }
    const [leaf] = this.app.workspace.getLeavesOfType(VIEW_TYPE_REMARGIN);
    if (leaf) {
      this.app.workspace.revealLeaf(leaf);
    }
  }

  /**
   * Stable accessor for "the most recently used markdown editor." Returns
   * null only when there is no markdown file open at all -- it does NOT
   * return null just because focus moved to the sidebar. Used by the `+`
   * button's reactive disabled state and by the `Add comment at cursor` command.
   */
  getLastMarkdownView(): MarkdownView | null {
    if (this.lastMarkdownView && this.lastMarkdownView.file) {
      return this.lastMarkdownView;
    }
    return null;
  }

  /**
   * Register (or clear) the React sidebar's handler for compose requests.
   * If a compose request arrived before the handler was ready, it is drained
   * synchronously here so the composer opens on the next tick.
   */
  setComposeHandler(handler: ((request: ComposeRequest) => void) | null) {
    this.composeHandler = handler;
    if (handler && this.pendingCompose) {
      const pending = this.pendingCompose;
      this.pendingCompose = null;
      handler(pending);
    }
  }

  /**
   * Ask the React sidebar to open its inline composer. If the sidebar is not
   * mounted yet (command fired while the sidebar was closed), the request
   * is stashed in `pendingCompose` and drained on the next handler
   * registration.
   */
  private requestCompose(request: ComposeRequest) {
    if (this.composeHandler) {
      this.composeHandler(request);
    } else {
      this.pendingCompose = request;
    }
  }

  /**
   * Shared entry point for "add a comment at the cursor." Both the `+`
   * button and the `Add comment at cursor` command route through here so the two
   * paths can never drift apart.
   */
  async addComment() {
    const view = this.getLastMarkdownView();
    if (!view || !view.file) {
      new Notice("Open a markdown file to add a comment");
      return;
    }
    const file = view.file;
    const cursorLine1 = view.editor.getCursor().line + 1;
    const lines = view.editor.getValue().split("\n");
    const snapped = snapAfterCommentBlock(lines, cursorLine1);
    await this.activateView();
    this.requestCompose({ file: file.path, afterLine: snapped });
  }
}

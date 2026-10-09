/** The Obsidian plugin entry point: views, commands, settings and the editor extensions. */

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
import { collapseEffectBridge, commentWidgetPlugin } from "./editor/commentWidget";
import { remarginPostProcessor } from "./editor/readingModeProcessor";
import { BackendContext } from "./hooks/useBackend";
import { PluginContext } from "./hooks/usePlugin";
import { PortalContainerContext } from "./hooks/usePortalContainer";
import { detectNewUpdates, type ReleasesFetcher, type UpdateComponent } from "./lib/githubReleases";
import { snapAfterCommentBlock } from "./lib/line-snap";
import { buildThreadTree } from "./lib/threadTree";
import { parseRemarginBlocks } from "./parser/parseRemarginBlocks";
import { CollapseState } from "./state/collapseState";
import { clampMarkdownScale, DEFAULT_SETTINGS, type RemarginSettings } from "./types";
import "./styles/globals.css";

/**
 * Detail payload for the `remargin:focus` event dispatched by
 * `RemarginPlugin.focusComment`. Subscribed to by `SidebarShell` so a
 * widget click in either editor surface can scroll + highlight the
 * matching sidebar card.
 */
export interface RemarginFocusDetail {
  commentId: string;
  file: string;
}

export const VIEW_TYPE_REMARGIN = "remargin-sidebar";

const UPDATE_NOTICE_MS = 8000;

const COMPONENT_LABELS: Record<UpdateComponent, string> = {
  plugin: "plugin",
  cli: "CLI",
};

/** Adapts Obsidian's CORS-free `requestUrl` to the update check's `ReleasesFetcher` shape. */
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

/** The sidebar view: mounts the React sidebar into its leaf. */
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

/** The settings tab: mounts the React settings UI. */
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
            onCheckUpdates: async () => {
              await this.plugin.runUpdateCheck(true);
              return this.plugin.settings;
            },
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

/** The plugin: owns settings, the backend, shared widget state and the sidebar bridges. */
export default class RemarginPlugin extends Plugin {
  settings: RemarginSettings = DEFAULT_SETTINGS;
  backend!: RemarginBackend;

  /**
   * Per-session collapse state for editor-side widget comments, shared by reading mode and Live
   * Preview. Created in `onload` and not persisted to plugin data.
   */
  collapseState!: CollapseState;

  /** Cached resolved identity the widgets read synchronously; `null` when none is configured. */
  currentIdentity: string | null = null;

  /** Plugin-scoped bus for sidebar-focus requests, kept out of other plugins' event namespace. */
  focusEvents!: EventTarget;

  /**
   * Most recently focused markdown view. `getActiveViewOfType(MarkdownView)` turns null the
   * moment the sidebar takes focus; this is cleared only when the cached view's file closes.
   */
  private lastMarkdownView: MarkdownView | null = null;

  private composeHandler: ((request: ComposeRequest) => void) | null = null;

  /** A compose request that arrived before the sidebar registered its handler. */
  private pendingCompose: ComposeRequest | null = null;

  private refreshHandler: (() => void) | null = null;

  /** Set when a refresh was requested before the sidebar registered its handler. */
  private pendingRefresh = false;

  async onload() {
    await this.loadSettings();
    this.applyMarkdownScale();

    const adapter = this.app.vault.adapter as unknown as { basePath?: string };
    const vaultPath = adapter.basePath ?? "";
    this.backend = new RemarginBackend(this.settings, vaultPath);

    // Created in `onload` so both reset whenever the plugin reloads.
    this.collapseState = new CollapseState();
    this.focusEvents = new EventTarget();

    this.addSettingTab(new RemarginSettingTab(this));

    // The Live Preview widget reads `settings.editorWidgets` on every build. `collapseEffectBridge`
    // turns `CollapseState` changes into CM6 transactions so chevron clicks rebuild at once.
    this.registerEditorExtension([commentWidgetPlugin(this), collapseEffectBridge(this)]);
    // The post-processor reads `settings.editorWidgets` on every render, so no re-registration.
    this.registerMarkdownPostProcessor(remarginPostProcessor(this));

    // A failure leaves `currentIdentity` null and the widgets on broadcast-pending semantics.
    void this.refreshIdentity();

    this.registerView(VIEW_TYPE_REMARGIN, (leaf) => new RemarginView(leaf, this));

    const initialView = this.app.workspace.getActiveViewOfType(MarkdownView);
    if (initialView) this.lastMarkdownView = initialView;

    // Set only on a non-null view, never to null, so the cache survives sidebar focus.
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
        void this.activateView();
        this.requestRefresh();
      },
    });

    this.addCommand({
      id: "expand-all-comments",
      name: "Expand all comments in document",
      callback: () => {
        void this.setAllRootsCollapsed(false);
      },
    });

    this.addCommand({
      id: "collapse-all-comments",
      name: "Collapse all comments in document",
      callback: () => {
        void this.setAllRootsCollapsed(true);
      },
    });

    this.addRibbonIcon("message-square", "Open Remargin", () => {
      this.activateView();
    });

    this.app.workspace.onLayoutReady(() => {
      this.activateView();
    });

    // Background only: a failure becomes the check-failed status, never an error Notice.
    void this.runUpdateCheck(false);
  }

  onunload(): void {
    // A disabled plugin leaves no styling hook on <body>.
    if (typeof document === "undefined") return;
    document.body.style.removeProperty("--remargin-md-scale");
  }

  /**
   * Run the GitHub-releases update check and persist the result. Fires a
   * single unobtrusive Notice per component that transitioned to
   * `update-available` since the last cached snapshot.
   *
   * Honors the `checkForUpdates` settings toggle: when off, no fetcher
   * is invoked and no Notice fires. `force=true` bypasses both the cache
   * TTL and the toggle (used by the Settings "Check now" button).
   *
   * Returns nothing — the caller reads `this.settings.updateCheck` for
   * the freshest snapshot (the SettingsTab re-reads settings through
   * `onSave` + display re-mount on the next open).
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
      // The backend wrapper swallows errors, but an unexpected bug here must not crash `onload`.
      return;
    }
    if (after === before) return;

    // Through saveSettings, so the backend's in-memory copy stays in sync.
    await this.saveSettings({ ...this.settings, updateCheck: after });

    // Notices fire only on the passive path: "Check now" renders its own inline status.
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
      // A stored `remarginMode` was never wired to the CLI: the vault's .remargin.yaml owns mode.
      if (saved && typeof saved === "object" && "remarginMode" in saved) {
        delete (saved as { remarginMode?: unknown }).remarginMode;
      }
      this.settings = Object.assign({}, DEFAULT_SETTINGS, saved);
      return;
    }
    // First run: use config mode if the CLI finds a human identity config above the vault.
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

    this.applyMarkdownScale();

    if (previousSide !== settings.sidebarSide) {
      for (const leaf of this.app.workspace.getLeavesOfType(VIEW_TYPE_REMARGIN)) {
        leaf.detach();
      }
      await this.activateView();
    }

    // A settings change may point at another identity config, so the cached identity is refreshed.
    void this.refreshIdentity();
  }

  /**
   * Write the current comment-markdown font scale to the `--remargin-md-scale`
   * CSS var on <body>. Both comment surfaces (sidebar + editor widgets)
   * live under <body>, so this one assignment restyles them together.
   */
  applyMarkdownScale(): void {
    // Headless unit tests drive `onload` without a document.
    if (typeof document === "undefined") return;
    document.body.style.setProperty(
      "--remargin-md-scale",
      String(clampMarkdownScale(this.settings.markdownScale))
    );
  }

  /**
   * Set the global comment-markdown font scale, persist it, and apply it
   * live. Shared by the sidebar +/-/reset control and the settings tab so
   * both paths clamp and persist identically.
   */
  async setMarkdownScale(scale: number): Promise<void> {
    await this.saveSettings({ ...this.settings, markdownScale: clampMarkdownScale(scale) });
  }

  /**
   * Drive every root comment in the active document to the given
   * collapsed state. No-op when no markdown view is active, the active
   * file can't be read, or the document has no parsable remargin blocks.
   * Used by the `expand-all-comments` / `collapse-all-comments` commands.
   */
  private async setAllRootsCollapsed(collapsed: boolean): Promise<void> {
    const view = this.app.workspace.getActiveViewOfType(MarkdownView);
    const file = view?.file;
    if (!file) return;
    let text: string;
    try {
      text = await this.app.vault.cachedRead(file);
    } catch {
      return;
    }
    const blocks = parseRemarginBlocks(text);
    const validComments = blocks
      .filter((b) => b.valid && b.comment.id)
      .map((b) => b.comment as { id: string; reply_to?: string });
    const trees = buildThreadTree(validComments as Parameters<typeof buildThreadTree>[0]);
    const rootIds = trees.map((n) => n.comment.id);
    if (rootIds.length === 0) return;
    this.collapseState.setMany(rootIds, collapsed);
  }

  /**
   * Resolve the active identity through the backend and stash it on
   * `currentIdentity` so the editor-side widgets can read it
   * synchronously. Failures (CLI missing, no config) leave the field
   * null — the widget falls back to broadcast-only auto-expand.
   */
  async refreshIdentity(): Promise<void> {
    try {
      const info = await this.backend.identity();
      this.currentIdentity = info.identity ?? null;
    } catch (err) {
      console.error("RemarginPlugin.refreshIdentity failed:", err);
      this.currentIdentity = null;
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
   * Register (or clear) the React sidebar's handler for refresh requests.
   * If a refresh was requested before the handler was ready, the pending
   * flag is drained synchronously here so the sidebar refetches on mount.
   */
  setRefreshHandler(handler: (() => void) | null) {
    this.refreshHandler = handler;
    if (handler && this.pendingRefresh) {
      this.pendingRefresh = false;
      handler();
    }
  }

  /**
   * Ask the React sidebar to refetch every section. If the sidebar is
   * not mounted yet (command fired while the sidebar was closed), the
   * request is stashed and drained on the next `setRefreshHandler` call.
   */
  private requestRefresh() {
    if (this.refreshHandler) {
      this.refreshHandler();
    } else {
      this.pendingRefresh = true;
    }
  }

  /**
   * Ask the sidebar to scroll to (and briefly highlight) the matching
   * comment card. Fires a `remargin:focus` `CustomEvent` on the plugin's
   * own `focusEvents` bus; `SidebarShell` is the canonical subscriber
   * and decides whether to switch the active-file filter, scroll, or
   * silently drop the request. When nothing is subscribed (sidebar
   * closed, no view mounted) the dispatch is a no-op — no exception,
   * no console noise — by EventTarget contract.
   */
  focusComment(commentId: string, file: string): void {
    this.focusEvents.dispatchEvent(
      new CustomEvent<RemarginFocusDetail>("remargin:focus", {
        detail: { commentId, file },
      })
    );
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

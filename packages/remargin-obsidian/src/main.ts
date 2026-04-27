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
import { CollapseState } from "./state/collapseState";
import { DEFAULT_SETTINGS, type RemarginSettings } from "./types";
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

export default class RemarginPlugin extends Plugin {
  settings: RemarginSettings = DEFAULT_SETTINGS;
  backend!: RemarginBackend;

  /**
   * Per-session collapse state for editor-side widget comments. Owned by
   * the plugin so reading mode (T37) and Live Preview (T38) can both
   * subscribe and stay in sync. Created in `onload`. Reset on plugin
   * reload — not persisted to plugin data (per-session scope, T36 spec).
   */
  collapseState!: CollapseState;

  /**
   * Plugin-scoped event bus for sidebar-focus requests. `focusComment`
   * dispatches `remargin:focus` here, and the React `SidebarShell`
   * subscribes/unsubscribes on mount/unmount. Picked over the workspace
   * event surface so the bridge does not leak into other plugins'
   * namespace. Created in `onload`.
   */
  focusEvents!: EventTarget;

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

  /** Registered by RemarginSidebar on mount; called by `requestRefresh`. */
  private refreshHandler: (() => void) | null = null;

  /**
   * `true` when a refresh was requested before the React sidebar registered
   * its handler (e.g. the command was fired while the sidebar was closed).
   * Drained on the next `setRefreshHandler` call.
   */
  private pendingRefresh = false;

  async onload() {
    await this.loadSettings();

    const adapter = this.app.vault.adapter as unknown as { basePath?: string };
    const vaultPath = adapter.basePath ?? "";
    this.backend = new RemarginBackend(this.settings, vaultPath);

    // Pretty-print foundation (T36): per-session collapse store + focus
    // bus, both consumed by the editor-side widgets shipped in T37/T38.
    // Created here so reload semantics match the design (state resets
    // when the plugin reloads).
    this.collapseState = new CollapseState();
    this.focusEvents = new EventTarget();

    this.addSettingTab(new RemarginSettingTab(this));

    // T38: pretty-print Live Preview widget. The plugin closure reads
    // `settings.editorWidgets` and the live-preview class on every
    // `build()`, so toggling the setting or flipping editor modes
    // takes effect on the next document change.
    //
    // The companion `collapseEffectBridge` ViewPlugin is the rem-jq30
    // Bug B fix: it adapts the plugin-wide `CollapseState` store into
    // CM6 transactions so the StateField can rebuild on chevron clicks
    // without waiting for a doc change.
    this.registerEditorExtension([commentWidgetPlugin(this), collapseEffectBridge(this)]);
    // T37: pretty-print reading-mode widget. The post-processor reads
    // `settings.editorWidgets` on every render call, so toggling the
    // setting at runtime takes effect on the next render — no need to
    // re-register.
    this.registerMarkdownPostProcessor(remarginPostProcessor(this));

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
        // Open the sidebar if it isn't already, then ask it to refetch.
        // If the sidebar is closed, `requestRefresh` stashes the request
        // and the sidebar drains it on its next `setRefreshHandler` call
        // (mirrors the compose-handler pattern).
        void this.activateView();
        this.requestRefresh();
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
   *
   * Wired in T36; consumed by T37 (reading-mode widget) and T38 (Live
   * Preview CM6 widget) when a user clicks an editor-side comment
   * widget.
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

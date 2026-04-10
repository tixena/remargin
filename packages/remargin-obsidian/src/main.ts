import { ItemView, Plugin, PluginSettingTab, type WorkspaceLeaf } from "obsidian";
import { createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { RemarginBackend } from "./backend";
import { RemarginSidebar } from "./components/RemarginSidebar";
import { SettingsTab } from "./components/settings/SettingsTab";
import { BackendContext } from "./hooks/useBackend";
// Editor extensions disabled — see rem-359. Re-enable after fixing the
// visual regression they cause in Live Preview and reading mode.
// import { commentWidgetPlugin } from "./editor/commentWidget";
// import { remarginPostProcessor } from "./editor/readingModeProcessor";
import { DEFAULT_SETTINGS, type RemarginSettings } from "./types";
import "./styles/globals.css";

export const VIEW_TYPE_REMARGIN = "remargin-sidebar";

class RemarginView extends ItemView {
  private root: Root | null = null;

  constructor(
    leaf: WorkspaceLeaf,
    private plugin: RemarginPlugin
  ) {
    super(leaf);
  }

  async onOpen() {
    const container = this.containerEl.children[1];
    container.empty();
    container.addClass("remargin-container");
    this.root = createRoot(container);
    this.root.render(
      createElement(
        BackendContext.Provider,
        { value: this.plugin.backend },
        createElement(RemarginSidebar, { plugin: this.plugin })
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
      createElement(SettingsTab, {
        settings: this.plugin.settings,
        onSave: (s: RemarginSettings) => this.plugin.saveSettings(s),
      })
    );
  }

  hide() {
    this.root?.unmount();
    this.root = null;
  }
}

export default class RemarginPlugin extends Plugin {
  settings: RemarginSettings = DEFAULT_SETTINGS;
  backend!: RemarginBackend;

  async onload() {
    await this.loadSettings();

    const adapter = this.app.vault.adapter as unknown as { basePath?: string };
    const vaultPath = adapter.basePath ?? "";
    this.backend = new RemarginBackend(this.settings, vaultPath);

    this.addSettingTab(new RemarginSettingTab(this));

    // Editor extensions (CM6 widget + reading-mode processor) are
    // disabled — see rem-359. They caused visual regressions in the
    // markdown editor. Re-enable once rewritten.
    // this.registerEditorExtension([commentWidgetPlugin]);
    // this.registerMarkdownPostProcessor(remarginPostProcessor);

    this.registerView(VIEW_TYPE_REMARGIN, (leaf) => new RemarginView(leaf, this));

    // Commands
    this.addCommand({
      id: "open-sidebar",
      name: "Open sidebar",
      callback: () => this.activateView(),
    });

    this.addCommand({
      id: "new-comment",
      name: "New comment at cursor",
      editorCallback: (editor) => {
        const file = this.app.workspace.getActiveFile();
        if (!file) return;
        const line = editor.getCursor().line + 1;
        this.activateView();
        // TODO: pass line context to sidebar prompt section
        console.log(`New comment at line ${line} in ${file.path}`);
      },
    });

    this.addCommand({
      id: "refresh",
      name: "Refresh comments",
      callback: () => {
        // Trigger a refresh by re-activating the view
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
        // Find the comment block at this line
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
  }

  async loadSettings() {
    const saved = await this.loadData();
    if (saved) {
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
}

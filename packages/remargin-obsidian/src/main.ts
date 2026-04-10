import { Plugin, PluginSettingTab, ItemView, WorkspaceLeaf } from "obsidian";
import { createRoot, Root } from "react-dom/client";
import { createElement } from "react";
import { RemarginSidebar } from "./components/RemarginSidebar";
import { SettingsTab } from "./components/settings/SettingsTab";
import { RemarginBackend } from "./backend";
import { BackendContext } from "./hooks/useBackend";
import { commentWidgetPlugin } from "./editor/commentWidget";
import { remarginPostProcessor } from "./editor/readingModeProcessor";
import { DEFAULT_SETTINGS, type RemarginSettings } from "./types";
import "./styles/globals.css";

export const VIEW_TYPE_REMARGIN = "remargin-sidebar";

class RemarginView extends ItemView {
  private root: Root | null = null;

  constructor(leaf: WorkspaceLeaf, private plugin: RemarginPlugin) {
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

    const vaultPath = (this.app.vault.adapter as any).basePath || "";
    this.backend = new RemarginBackend(this.settings, vaultPath);

    this.addSettingTab(new RemarginSettingTab(this));

    // Register CM6 editor extension for Live Preview mode
    this.registerEditorExtension([commentWidgetPlugin]);

    // Register reading mode post-processor
    this.registerMarkdownPostProcessor(remarginPostProcessor);

    this.registerView(
      VIEW_TYPE_REMARGIN,
      (leaf) => new RemarginView(leaf, this)
    );

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
        const block = blocks.find(
          (b) => line >= b.startLine && line <= b.endLine
        );
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
    this.settings = Object.assign(
      {},
      DEFAULT_SETTINGS,
      await this.loadData()
    );
  }

  async saveSettings(settings: RemarginSettings) {
    this.settings = settings;
    this.backend?.updateSettings(settings);
    await this.saveData(settings);
  }

  async activateView() {
    const leaves = this.app.workspace.getLeavesOfType(VIEW_TYPE_REMARGIN);
    if (leaves.length === 0) {
      const leaf = this.app.workspace.getRightLeaf(false);
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

import { Plugin, PluginSettingTab, ItemView, WorkspaceLeaf } from "obsidian";
import { createRoot, Root } from "react-dom/client";
import { createElement } from "react";
import { RemarginSidebar } from "./components/RemarginSidebar";
import { SettingsTab } from "./components/settings/SettingsTab";
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
    this.root.render(createElement(RemarginSidebar, { plugin: this.plugin }));
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

  async onload() {
    await this.loadSettings();

    this.addSettingTab(new RemarginSettingTab(this));

    this.registerView(
      VIEW_TYPE_REMARGIN,
      (leaf) => new RemarginView(leaf, this)
    );

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

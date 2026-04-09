import { Plugin, ItemView, WorkspaceLeaf } from "obsidian";
import { createRoot, Root } from "react-dom/client";
import { createElement } from "react";
import { RemarginSidebar } from "./components/RemarginSidebar";
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

export default class RemarginPlugin extends Plugin {
  async onload() {
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

/** "flat" renders a single-level list; "tree" groups files by directory. */
export type ViewMode = "flat" | "tree";

export interface RemarginSettings {
  remarginPath: string;
  claudePath: string;
  workingDirectory: string;
  identityMode: "config" | "manual";
  configFilePath: string;
  authorName: string;
  keyFilePath: string;
  remarginMode: string;
  sidebarSide: "left" | "right";
  /** Per-section view mode, persisted across sessions (UI task 26). */
  sandboxView: ViewMode;
  inboxView: ViewMode;
}

export const DEFAULT_SETTINGS: RemarginSettings = {
  remarginPath: "remargin",
  claudePath: "claude",
  workingDirectory: "",
  identityMode: "manual",
  configFilePath: "",
  authorName: "user",
  keyFilePath: "",
  remarginMode: "open",
  sidebarSide: "left",
  sandboxView: "tree",
  inboxView: "tree",
};

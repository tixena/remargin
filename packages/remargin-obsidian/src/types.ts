import type { UpdateCheckState } from "./lib/githubReleases";

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
  sidebarSide: "left" | "right";
  /** Per-section view mode, persisted across sessions (UI task 26). */
  sandboxView: ViewMode;
  inboxView: ViewMode;
  /**
   * User toggle for the GitHub-releases update probe. When `false` the
   * plugin performs zero network calls and skips the startup Notice
   * entirely — no silent heartbeat. Default on to match the "check and
   * tell me" expectation most users have for dev-tool plugins.
   */
  checkForUpdates: boolean;
  /**
   * Cached result of the last successful (or failed) update probe. Not
   * surfaced in the settings UI — the Updates section renders this
   * through the backend's read accessor. `undefined` on first install
   * and after a reset, which forces the next `onload` to fetch.
   */
  updateCheck?: UpdateCheckState;
}

export const DEFAULT_SETTINGS: RemarginSettings = {
  remarginPath: "remargin",
  claudePath: "claude",
  workingDirectory: "",
  identityMode: "manual",
  configFilePath: "",
  authorName: "user",
  keyFilePath: "",
  sidebarSide: "left",
  sandboxView: "tree",
  inboxView: "tree",
  checkForUpdates: true,
};

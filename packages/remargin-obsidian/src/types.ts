/** Plugin settings: their shape, defaults and bounds. */

import type { UpdateCheckState } from "./lib/githubReleases";

/** "flat" renders a single-level list; "tree" groups files by directory. */
export type ViewMode = "flat" | "tree";

/**
 * Inbox filter mode. Each value maps to one server-side `remargin query`
 * narrowing (see `inboxFilterQueryOpts`): comments waiting on me, my own
 * still-pending comments, unacked broadcasts, the broad pending
 * predicate, and everything.
 */
export type InboxFilter = "for-me" | "from-me" | "unassigned" | "pending" | "all";

/** Everything the plugin persists in its data file. */
export interface RemarginSettings {
  remarginPath: string;
  claudePath: string;
  /** Terminal argv prefix for sandbox Submit, e.g. `ptyxis --`; empty means auto-detect per OS. */
  terminalCommand: string;
  workingDirectory: string;
  identityMode: "config" | "manual";
  configFilePath: string;
  authorName: string;
  keyFilePath: string;
  sidebarSide: "left" | "right";
  sandboxView: ViewMode;
  inboxView: ViewMode;
  inboxFilter: InboxFilter;
  /** When false the plugin makes no network calls and shows no startup Notice. */
  checkForUpdates: boolean;
  /** The last update probe's result; `undefined` forces the next `onload` to fetch. */
  updateCheck?: UpdateCheckState;
  /**
   * When true, remargin fenced blocks render as rich, read-only widgets in Live Preview and
   * reading mode. Editing always happens in the sidebar. Off by default.
   */
  editorWidgets: boolean;
  /**
   * One font-scale multiplier for rendered comment markdown in the sidebar and the editor
   * widgets, applied as the `--remargin-md-scale` CSS var.
   */
  markdownScale: number;
}

/** Bounds for {@link RemarginSettings.markdownScale}. */
export const MARKDOWN_SCALE_MIN = 0.7;
export const MARKDOWN_SCALE_MAX = 2;
export const MARKDOWN_SCALE_STEP = 0.1;
export const MARKDOWN_SCALE_DEFAULT = 1;

/** Clamp to the allowed range and snap float drift to one decimal. */
export function clampMarkdownScale(value: number): number {
  if (!Number.isFinite(value)) return MARKDOWN_SCALE_DEFAULT;
  const clamped = Math.min(MARKDOWN_SCALE_MAX, Math.max(MARKDOWN_SCALE_MIN, value));
  return Math.round(clamped * 100) / 100;
}

export const DEFAULT_SETTINGS: RemarginSettings = {
  remarginPath: "remargin",
  claudePath: "claude",
  terminalCommand: "",
  workingDirectory: "",
  identityMode: "manual",
  configFilePath: "",
  authorName: "user",
  keyFilePath: "",
  sidebarSide: "left",
  sandboxView: "tree",
  inboxView: "tree",
  inboxFilter: "for-me",
  checkForUpdates: true,
  editorWidgets: false,
  markdownScale: MARKDOWN_SCALE_DEFAULT,
};

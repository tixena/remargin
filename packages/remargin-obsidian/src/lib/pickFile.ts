/** The native file-open dialog, reached through Electron when it is available. */

import { dirname } from "node:path";

/**
 * File-type filter entry for the native open dialog. Matches Electron's
 * `FileFilter` shape so callers that only depend on the filter contract do
 * not need to import Electron types (Electron is optional at runtime).
 */
export interface FilePickerFilter {
  name: string;
  /** Extensions without the leading dot; use `["*"]` for "All Files". */
  extensions: string[];
}

/** Options for {@link pickFile}. */
export interface PickFileOptions {
  /** Always include an "All Files" entry: signing keys, for one, often have no extension. */
  filters?: FilePickerFilter[];
  /** A file path here opens the dialog in its parent directory. */
  defaultPath?: string;
  title?: string;
}

/** The subset of Electron's open-dialog options this module passes. */
interface ElectronOpenDialogOptions {
  filters?: FilePickerFilter[];
  defaultPath?: string;
  properties?: string[];
  title?: string;
}

/** The subset of Electron's open-dialog result this module reads. */
interface ElectronOpenDialogResult {
  canceled: boolean;
  filePaths: string[];
}

/** The one method of Electron's `dialog` module this module calls. */
interface ElectronDialogModule {
  showOpenDialog: (
    options: ElectronOpenDialogOptions
  ) => Promise<ElectronOpenDialogResult>;
}

/** Node's `require`, typed for the runtime probe. */
interface NodeRequire {
  (id: string): unknown;
}

/**
 * Look up Electron's `dialog` module at runtime. Obsidian ships Electron, so
 * `require` is available, but different Obsidian versions expose the dialog
 * either through `@electron/remote` (newer) or `electron.remote` (older).
 * We probe both and return `null` when neither is reachable so callers can
 * degrade gracefully (hide the Browse button instead of crashing).
 */
function resolveElectronDialog(): ElectronDialogModule | null {
  const req = (globalThis as { require?: NodeRequire }).require;
  if (typeof req !== "function") return null;

  // Newer Electron: dialog lives behind @electron/remote.
  try {
    const mod = req("@electron/remote") as { dialog?: ElectronDialogModule };
    if (mod.dialog?.showOpenDialog) return mod.dialog;
  } catch {
    // module not present — fall through to legacy probe
  }

  // Legacy Electron (<14): dialog is on electron.remote.
  try {
    const mod = req("electron") as {
      remote?: { dialog?: ElectronDialogModule };
    };
    if (mod.remote?.dialog?.showOpenDialog) return mod.remote.dialog;
  } catch {
    // module not present — report unavailable
  }

  return null;
}

/**
 * Returns `true` when the native file-open dialog is reachable. Components
 * can call this at render time to hide the Browse button on hosts that do
 * not expose Electron's dialog API (future mobile Obsidian builds, unit
 * test environments, etc.).
 */
export function isFilePickerAvailable(): boolean {
  return resolveElectronDialog() !== null;
}

/**
 * Open the OS native file-open dialog and return the selected file's path.
 * Returns `null` when the user cancels the dialog OR when Electron's dialog
 * API is unavailable — callers treat both cases as "no change".
 *
 * The dialog is always single-select (`properties: ["openFile"]`). If a
 * `defaultPath` that looks like a file is passed, the parent directory is
 * used as the starting point so the dialog opens "next to" the existing
 * value instead of inside a nonexistent file path.
 */
export async function pickFile(options: PickFileOptions = {}): Promise<string | null> {
  const dialog = resolveElectronDialog();
  if (!dialog) return null;

  const defaultPath = options.defaultPath
    ? deriveDefaultPath(options.defaultPath)
    : undefined;

  const result = await dialog.showOpenDialog({
    properties: ["openFile"],
    filters: options.filters,
    defaultPath,
    title: options.title,
  });

  if (result.canceled || result.filePaths.length === 0) return null;
  return result.filePaths[0] ?? null;
}

/**
 * Convert a user-provided `defaultPath` into something Electron can use.
 * When the path looks like a file (contains a dot after the last separator),
 * we hand Electron the parent directory so the dialog opens in that folder;
 * otherwise the value is passed through unchanged.
 */
function deriveDefaultPath(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return trimmed;
  const lastSep = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  const lastSegment = lastSep >= 0 ? trimmed.slice(lastSep + 1) : trimmed;
  if (lastSegment.includes(".")) {
    return dirname(trimmed);
  }
  return trimmed;
}

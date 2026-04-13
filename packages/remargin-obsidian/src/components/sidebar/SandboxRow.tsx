import { FileText, Trash2 } from "lucide-react";
import { Checkbox } from "@/components/ui/checkbox";

export type SandboxRowVariant = "staged" | "unstaged";

export interface SandboxRowProps {
  /** Vault-relative file path. */
  path: string;
  /** Indent depth (tree view passes >0; flat view passes 0). */
  depth?: number;
  /** Controls which affordances render (checkbox + trash only for unstaged). */
  variant: SandboxRowVariant;
  /** Whether this row is currently selected for bulk actions. */
  selected: boolean;
  /** Toggle the bulk-action selection for this row. */
  onToggleSelected: (path: string) => void;
  /** Open the file in the editor. */
  onOpenFile: (path: string) => void;
  /** Remove the file from the sandbox (unstaged variant only). */
  onRemoveFile?: (path: string) => void;
}

/**
 * Unified row renderer for the Sandbox sub-groups. The staged variant shows a
 * file icon + path only (no per-row actions, per task 24's decision to avoid
 * accidental unstages). The unstaged variant shows a selection checkbox plus
 * a trailing trash icon that drops the file from the persistent sandbox.
 */
export function SandboxRow({
  path,
  depth = 0,
  variant,
  selected,
  onToggleSelected,
  onOpenFile,
  onRemoveFile,
}: SandboxRowProps) {
  const paddingLeft = `${24 + depth * 16}px`;
  const name = path.split("/").pop() ?? path;

  if (variant === "staged") {
    return (
      <div
        className="group flex items-center gap-1.5 py-1 pr-4 hover:bg-bg-hover"
        style={{ paddingLeft }}
      >
        <FileText className="w-3 h-3 text-text-faint shrink-0" />
        <button
          type="button"
          className="flex-1 text-xs font-mono text-text-muted truncate text-left hover:text-text-normal"
          onClick={() => onOpenFile(path)}
          title={path}
        >
          {name}
        </button>
      </div>
    );
  }

  return (
    <div
      className="group flex items-center gap-1.5 py-1 pr-4 hover:bg-bg-hover"
      style={{ paddingLeft }}
    >
      <Checkbox
        checked={selected}
        onCheckedChange={() => onToggleSelected(path)}
        className="w-3 h-3"
      />
      <FileText className="w-3 h-3 text-text-faint shrink-0" />
      <button
        type="button"
        className="flex-1 text-xs font-mono text-text-muted truncate text-left hover:text-text-normal"
        onClick={() => onOpenFile(path)}
        title={path}
      >
        {name}
      </button>
      {onRemoveFile && (
        <button
          type="button"
          className="hidden group-hover:flex items-center justify-center w-4 h-4 shrink-0 text-text-faint hover:text-red-400"
          onClick={() => onRemoveFile(path)}
          title="Remove from sandbox"
        >
          <Trash2 className="w-2.5 h-2.5" />
        </button>
      )}
    </div>
  );
}

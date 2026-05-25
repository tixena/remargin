import { Trash2 } from "lucide-react";
import { Checkbox } from "@/components/ui/checkbox";

export type SandboxRowVariant = "staged" | "unstaged";

export interface SandboxRowProps {
  /** Vault-relative file path. */
  path: string;
  /** Indent depth (tree view passes >0; flat view passes 0). */
  depth?: number;
  /** Controls which affordances render (trash only for unstaged). */
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
 * Unified row renderer for the Sandbox sub-groups. Selection checkbox +
 * filename; the unstaged variant also gets a trailing trash icon (on
 * hover) that drops the file from the persistent sandbox.
 *
 * The leading file icon was removed in T-redesign: filenames carry their
 * own extension hint and the icon was visual noise at this density.
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
  const name = path.split("/").pop() ?? path;

  return (
    <div className="rmg-sandbox-row group">
      <Checkbox checked={selected} onCheckedChange={() => onToggleSelected(path)} />
      <button
        type="button"
        className="rmg-sandbox-row__name"
        onClick={() => onOpenFile(path)}
        title={path}
      >
        {name}
      </button>
      {variant === "unstaged" && onRemoveFile && (
        <button
          type="button"
          className="rmg-sandbox-row__remove"
          onClick={() => onRemoveFile(path)}
          title="Remove from sandbox"
          aria-label="Remove from sandbox"
        >
          <Trash2 />
        </button>
      )}
    </div>
  );
}

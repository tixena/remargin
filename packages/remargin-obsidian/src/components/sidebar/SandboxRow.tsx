/** One file row of a sandbox sub-group. */

import { Trash2 } from "lucide-react";
import { Checkbox } from "@/components/ui/checkbox";

/** Which sub-group a row belongs to; only unstaged rows get the trash action. */
export type SandboxRowVariant = "staged" | "unstaged";

/** Props for {@link SandboxRow}. */
export interface SandboxRowProps {
  path: string;
  depth?: number;
  variant: SandboxRowVariant;
  selected: boolean;
  onToggleSelected: (path: string) => void;
  onOpenFile: (path: string) => void;
  onRemoveFile?: (path: string) => void;
}

/**
 * Unified row renderer for the Sandbox sub-groups. Selection checkbox +
 * filename; the unstaged variant also gets a trailing trash icon (on
 * hover) that drops the file from the persistent sandbox.
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

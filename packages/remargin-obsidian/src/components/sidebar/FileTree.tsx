import { ChevronDown, ChevronRight, FileText, Folder, X } from "lucide-react";
import { useCallback, useState } from "react";
import { Checkbox } from "@/components/ui/checkbox";
import { buildFileTree, type FileTreeNode } from "@/lib/buildFileTree";

export interface FileTreeProps {
  files: string[];
  staged: Set<string>;
  onToggleStaged: (path: string) => void;
  onOpenFile: (path: string) => void;
  onRemoveFile?: (path: string) => void;
}

/**
 * Compute the checked state for a directory node based on its leaf descendants.
 * Returns `true` (all staged), `false` (none staged), or `"indeterminate"`.
 */
function dirCheckedState(node: FileTreeNode, staged: Set<string>): boolean | "indeterminate" {
  const leaves = collectLeaves(node);
  if (leaves.length === 0) return false;
  const stagedCount = leaves.filter((p) => staged.has(p)).length;
  if (stagedCount === 0) return false;
  if (stagedCount === leaves.length) return true;
  return "indeterminate";
}

/** Recursively collect all leaf (file) fullPaths under a node. */
function collectLeaves(node: FileTreeNode): string[] {
  if (!node.isDir) return [node.fullPath];
  const result: string[] = [];
  for (const child of node.children) {
    result.push(...collectLeaves(child));
  }
  return result;
}

function DirectoryNode({
  node,
  depth,
  staged,
  onToggleStaged,
  onOpenFile,
  onRemoveFile,
}: {
  node: FileTreeNode;
  depth: number;
  staged: Set<string>;
  onToggleStaged: (path: string) => void;
  onOpenFile: (path: string) => void;
  onRemoveFile?: (path: string) => void;
}) {
  const [expanded, setExpanded] = useState(true);
  const checked = dirCheckedState(node, staged);

  const handleToggleDir = useCallback(() => {
    const leaves = collectLeaves(node);
    // If all are staged, unstage all. Otherwise stage all.
    const allStaged = leaves.every((p) => staged.has(p));
    for (const leaf of leaves) {
      if (allStaged && staged.has(leaf)) {
        onToggleStaged(leaf);
      } else if (!allStaged && !staged.has(leaf)) {
        onToggleStaged(leaf);
      }
    }
  }, [node, staged, onToggleStaged]);

  return (
    <>
      <div
        className="flex items-center gap-2 py-1 hover:bg-bg-hover cursor-pointer"
        style={{ paddingLeft: `${16 + depth * 16}px`, paddingRight: "16px" }}
        onClick={() => setExpanded((prev) => !prev)}
      >
        {expanded ? (
          <ChevronDown className="w-3 h-3 text-text-faint shrink-0" />
        ) : (
          <ChevronRight className="w-3 h-3 text-text-faint shrink-0" />
        )}
        <Checkbox
          checked={checked}
          onCheckedChange={() => handleToggleDir()}
          onClick={(e) => e.stopPropagation()}
          className="w-3.5 h-3.5"
        />
        <Folder className="w-3 h-3 text-text-faint shrink-0" />
        <span className="text-xs font-mono text-text-muted truncate">{node.name}</span>
      </div>
      {expanded &&
        node.children.map((child) => (
          <TreeNode
            key={child.fullPath}
            node={child}
            depth={depth + 1}
            staged={staged}
            onToggleStaged={onToggleStaged}
            onOpenFile={onOpenFile}
            onRemoveFile={onRemoveFile}
          />
        ))}
    </>
  );
}

function FileNode({
  node,
  depth,
  staged,
  onToggleStaged,
  onOpenFile,
  onRemoveFile,
}: {
  node: FileTreeNode;
  depth: number;
  staged: Set<string>;
  onToggleStaged: (path: string) => void;
  onOpenFile: (path: string) => void;
  onRemoveFile?: (path: string) => void;
}) {
  return (
    <div
      className="group flex items-center gap-2 py-1 hover:bg-bg-hover"
      style={{ paddingLeft: `${16 + depth * 16}px`, paddingRight: "16px" }}
    >
      {/* Spacer to align with directory chevrons */}
      <div className="w-3 shrink-0" />
      <Checkbox
        checked={staged.has(node.fullPath)}
        onCheckedChange={() => onToggleStaged(node.fullPath)}
        className="w-3.5 h-3.5"
      />
      <FileText className="w-3 h-3 text-text-faint shrink-0" />
      <button
        type="button"
        className="flex-1 text-xs font-mono text-text-muted truncate text-left hover:text-text-normal"
        onClick={() => onOpenFile(node.fullPath)}
      >
        {node.name}
      </button>
      {onRemoveFile && (
        <button
          type="button"
          className="hidden group-hover:flex items-center justify-center w-4 h-4 shrink-0 text-text-faint hover:text-red-400"
          onClick={() => onRemoveFile(node.fullPath)}
          title="Remove from sandbox"
        >
          <X className="w-3 h-3" />
        </button>
      )}
    </div>
  );
}

function TreeNode({
  node,
  depth,
  staged,
  onToggleStaged,
  onOpenFile,
  onRemoveFile,
}: {
  node: FileTreeNode;
  depth: number;
  staged: Set<string>;
  onToggleStaged: (path: string) => void;
  onOpenFile: (path: string) => void;
  onRemoveFile?: (path: string) => void;
}) {
  if (node.isDir) {
    return (
      <DirectoryNode
        node={node}
        depth={depth}
        staged={staged}
        onToggleStaged={onToggleStaged}
        onOpenFile={onOpenFile}
        onRemoveFile={onRemoveFile}
      />
    );
  }
  return (
    <FileNode
      node={node}
      depth={depth}
      staged={staged}
      onToggleStaged={onToggleStaged}
      onOpenFile={onOpenFile}
      onRemoveFile={onRemoveFile}
    />
  );
}

/**
 * Hierarchical file tree view for the Sandbox section. Renders collapsible
 * directories with tri-state checkboxes and indented file leaves.
 */
export function FileTree({
  files,
  staged,
  onToggleStaged,
  onOpenFile,
  onRemoveFile,
}: FileTreeProps) {
  const tree = buildFileTree(files);

  return (
    <div className="flex flex-col">
      {tree.map((node) => (
        <TreeNode
          key={node.fullPath}
          node={node}
          depth={0}
          staged={staged}
          onToggleStaged={onToggleStaged}
          onOpenFile={onOpenFile}
          onRemoveFile={onRemoveFile}
        />
      ))}
    </div>
  );
}

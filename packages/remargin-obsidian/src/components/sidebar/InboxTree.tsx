import {
  Check,
  ChevronDown,
  ChevronRight,
  Clock,
  FileText,
  Folder,
  MoreHorizontal,
} from "lucide-react";
import { useState } from "react";
import { deriveLeafState } from "@/components/sidebar/inboxLeafState";
import { MarkdownContent } from "@/components/sidebar/MarkdownContent";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ExpandedComment } from "@/generated";
import { useParticipants } from "@/hooks/useParticipants";
import { authorLabel } from "@/lib/authorLabel";
import { buildFileTree, type FileTreeNode } from "@/lib/buildFileTree";

interface InboxItem {
  file: string;
  comment: ExpandedComment;
}

interface InboxTreeProps {
  items: InboxItem[];
  /**
   * Current identity. `null` while the CLI identity probe is still in
   * flight — leaves render as neutral in that window.
   */
  me: string | null;
  onOpenAtLine?: (filePath: string, line?: number) => void;
  onAck?: (file: string, id: string, remove: boolean) => void;
}

interface CommentLeafProps {
  item: InboxItem;
  depth: number;
  me: string | null;
  onOpenAtLine?: (filePath: string, line?: number) => void;
  onAck?: (file: string, id: string, remove: boolean) => void;
}

function formatRelativeTime(ts?: string): string {
  if (!ts) return "";
  try {
    const diff = Date.now() - new Date(ts).getTime();
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return "now";
    if (mins < 60) return `${mins}m`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h`;
    const days = Math.floor(hours / 24);
    return `${days}d`;
  } catch {
    return "";
  }
}

function CommentLeaf({ item, depth, me, onOpenAtLine, onAck }: CommentLeafProps) {
  const { resolveDisplayName } = useParticipants();
  const { label: authorDisplay, title: authorTitle } = authorLabel(
    item.comment.author,
    resolveDisplayName
  );
  const { visual, ackedByMe } = deriveLeafState(item.comment, me);
  const visualCls =
    visual === "me-directed-unacked"
      ? "border-l-2 border-l-purple-500 bg-purple-500/5 hover:bg-purple-500/10"
      : visual === "acked-by-me"
        ? "opacity-60"
        : "hover:bg-bg-hover";
  return (
    <div
      className={`flex flex-col gap-1 py-2 border-b border-bg-border cursor-pointer ${visualCls}`}
      style={{ paddingLeft: `${16 + depth * 16}px`, paddingRight: "16px" }}
      onClick={() => onOpenAtLine?.(item.file, item.comment.line)}
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 min-w-0">
          <Badge
            className={`px-1 py-0 text-[9px] font-semibold ${
              item.comment.author_type === "agent"
                ? "bg-purple-400 text-white"
                : "bg-blue-400 text-white"
            }`}
          >
            {item.comment.author_type === "agent" ? "AI" : "H"}
          </Badge>
          {item.comment.id && (
            <Badge className="px-1 py-0 text-[9px] font-mono font-semibold bg-slate-500 text-white">
              {item.comment.id}
            </Badge>
          )}
          {item.comment.line > 0 && (
            <span className="text-[9px] text-text-faint font-mono">L{item.comment.line}</span>
          )}
          <span className="text-xs font-medium text-text-normal truncate" title={authorTitle}>
            {authorDisplay}
          </span>
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {visual === "me-directed-unacked" && item.comment.id && onAck && (
            <Button
              size="sm"
              variant="outline"
              className="h-5 px-2 text-[10px] gap-1"
              aria-label="Ack this comment"
              onClick={(e) => {
                e.stopPropagation();
                onAck(item.file, item.comment.id, false);
              }}
            >
              <Check className="w-2.5 h-2.5" />
              Ack
            </Button>
          )}
          {ackedByMe && item.comment.id && onAck && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-5 w-5 p-0 text-text-faint"
                  aria-label="Inbox row actions"
                  onClick={(e) => e.stopPropagation()}
                >
                  <MoreHorizontal className="w-2.5 h-2.5" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  className="text-xs"
                  onClick={(e) => {
                    e.stopPropagation();
                    onAck(item.file, item.comment.id, true);
                  }}
                >
                  Unack
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          )}
          <Clock className="w-3 h-3 text-text-faint" />
          <span className="text-[10px] text-text-faint whitespace-nowrap">
            {formatRelativeTime(item.comment.ts)}
          </span>
        </div>
      </div>
      <div className="line-clamp-2 overflow-hidden">
        <MarkdownContent content={item.comment.content ?? ""} sourcePath={item.file} />
      </div>
    </div>
  );
}

interface FileNodeProps {
  filePath: string;
  comments: InboxItem[];
  depth: number;
  isActive: boolean;
  me: string | null;
  onOpenAtLine?: (filePath: string, line?: number) => void;
  onAck?: (file: string, id: string, remove: boolean) => void;
}

function InboxFileNode({
  filePath,
  comments,
  depth,
  isActive,
  me,
  onOpenAtLine,
  onAck,
}: FileNodeProps) {
  const [expanded, setExpanded] = useState(true);
  const pendingCount = comments.filter((i) => i.comment.ack?.length === 0).length;

  const handleClick = () => {
    setExpanded((prev) => !prev);
  };

  const fileName = filePath.split("/").pop() ?? filePath;

  return (
    <>
      <div
        className="flex items-center gap-2 py-1.5 hover:bg-bg-hover cursor-pointer border-b border-bg-border"
        style={{ paddingLeft: `${16 + depth * 16}px`, paddingRight: "16px" }}
        onClick={handleClick}
      >
        {expanded ? (
          <ChevronDown className="w-3 h-3 text-text-faint shrink-0" />
        ) : (
          <ChevronRight className="w-3 h-3 text-text-faint shrink-0" />
        )}
        <FileText className="w-3 h-3 text-text-faint shrink-0" />
        <span className="flex-1 text-xs font-mono text-text-muted truncate">{fileName}</span>
        {pendingCount > 0 && (
          <Badge className="px-1.5 py-0 text-[9px] font-semibold bg-accent text-white">
            {pendingCount}
          </Badge>
        )}
      </div>
      {expanded &&
        comments.map((item) => (
          <CommentLeaf
            key={`${item.file}:${item.comment.id}`}
            item={item}
            depth={depth + 1}
            me={me}
            onOpenAtLine={onOpenAtLine}
            onAck={onAck}
          />
        ))}
    </>
  );
}

interface DirNodeProps {
  node: FileTreeNode;
  depth: number;
  itemsByFile: Map<string, InboxItem[]>;
  activeFile?: string;
  me: string | null;
  onOpenAtLine?: (filePath: string, line?: number) => void;
  onAck?: (file: string, id: string, remove: boolean) => void;
}

function InboxDirNode({
  node,
  depth,
  itemsByFile,
  activeFile,
  me,
  onOpenAtLine,
  onAck,
}: DirNodeProps) {
  const [expanded, setExpanded] = useState(true);

  // Count pending comments under this directory
  const pendingCount = collectFileLeaves(node)
    .flatMap((fp) => itemsByFile.get(fp) ?? [])
    .filter((i) => i.comment.ack?.length === 0).length;

  return (
    <>
      <div
        className="flex items-center gap-2 py-1.5 hover:bg-bg-hover cursor-pointer border-b border-bg-border"
        style={{ paddingLeft: `${16 + depth * 16}px`, paddingRight: "16px" }}
        onClick={() => setExpanded((prev) => !prev)}
      >
        {expanded ? (
          <ChevronDown className="w-3 h-3 text-text-faint shrink-0" />
        ) : (
          <ChevronRight className="w-3 h-3 text-text-faint shrink-0" />
        )}
        <Folder className="w-3 h-3 text-text-faint shrink-0" />
        <span className="flex-1 text-xs font-mono text-text-muted truncate">{node.name}</span>
        {pendingCount > 0 && (
          <Badge className="px-1.5 py-0 text-[9px] font-semibold bg-accent/60 text-white">
            {pendingCount}
          </Badge>
        )}
      </div>
      {expanded &&
        node.children.map((child) => (
          <InboxTreeNode
            key={child.fullPath}
            node={child}
            depth={depth + 1}
            itemsByFile={itemsByFile}
            activeFile={activeFile}
            me={me}
            onOpenAtLine={onOpenAtLine}
            onAck={onAck}
          />
        ))}
    </>
  );
}

/** Collect all non-directory fullPaths from a tree node. */
function collectFileLeaves(node: FileTreeNode): string[] {
  if (!node.isDir) return [node.fullPath];
  const result: string[] = [];
  for (const child of node.children) {
    result.push(...collectFileLeaves(child));
  }
  return result;
}

interface InboxTreeNodeProps {
  node: FileTreeNode;
  depth: number;
  itemsByFile: Map<string, InboxItem[]>;
  activeFile?: string;
  me: string | null;
  onOpenAtLine?: (filePath: string, line?: number) => void;
  onAck?: (file: string, id: string, remove: boolean) => void;
}

function InboxTreeNode({
  node,
  depth,
  itemsByFile,
  activeFile,
  me,
  onOpenAtLine,
  onAck,
}: InboxTreeNodeProps) {
  if (node.isDir) {
    return (
      <InboxDirNode
        node={node}
        depth={depth}
        itemsByFile={itemsByFile}
        activeFile={activeFile}
        me={me}
        onOpenAtLine={onOpenAtLine}
        onAck={onAck}
      />
    );
  }
  const comments = itemsByFile.get(node.fullPath) ?? [];
  return (
    <InboxFileNode
      filePath={node.fullPath}
      comments={comments}
      depth={depth}
      isActive={activeFile === node.fullPath}
      me={me}
      onOpenAtLine={onOpenAtLine}
      onAck={onAck}
    />
  );
}

/**
 * Three-level tree view for the Inbox section.
 * Groups comments by: directory -> file -> comment leaf.
 */
export function InboxTree({ items, me, onOpenAtLine, onAck }: InboxTreeProps) {
  // Build a map from file path to sorted comments
  const itemsByFile = new Map<string, InboxItem[]>();
  for (const item of items) {
    let arr = itemsByFile.get(item.file);
    if (!arr) {
      arr = [];
      itemsByFile.set(item.file, arr);
    }
    arr.push(item);
  }
  // Sort comments within each file by timestamp descending
  for (const arr of itemsByFile.values()) {
    arr.sort((a, b) => (b.comment.ts ?? "").localeCompare(a.comment.ts ?? ""));
  }

  const files = Array.from(itemsByFile.keys());
  const tree = buildFileTree(files);

  return (
    <div className="flex flex-col">
      {tree.map((node) => (
        <InboxTreeNode
          key={node.fullPath}
          node={node}
          depth={0}
          itemsByFile={itemsByFile}
          me={me}
          onOpenAtLine={onOpenAtLine}
          onAck={onAck}
        />
      ))}
    </div>
  );
}

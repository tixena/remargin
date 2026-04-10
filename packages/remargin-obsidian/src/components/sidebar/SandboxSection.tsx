import { useState, useCallback } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ScrollArea } from "@/components/ui/scroll-area";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { FileText, FolderTree, List, Send } from "lucide-react";

interface SandboxSectionProps {
  touchedFiles: string[];
  onOpenFile?: (path: string) => void;
  onSubmit?: (stagedFiles: string[]) => void;
}

export function SandboxSection({
  touchedFiles,
  onOpenFile,
  onSubmit,
}: SandboxSectionProps) {
  const [staged, setStaged] = useState<Set<string>>(new Set(touchedFiles));
  const [viewMode, setViewMode] = useState<"flat" | "tree">("tree");

  const toggleStaged = useCallback((path: string) => {
    setStaged((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  const toggleAll = useCallback(() => {
    if (staged.size === touchedFiles.length) {
      setStaged(new Set());
    } else {
      setStaged(new Set(touchedFiles));
    }
  }, [staged.size, touchedFiles]);

  const handleSubmit = useCallback(() => {
    const files = touchedFiles.filter((f) => staged.has(f));
    onSubmit?.(files);
  }, [touchedFiles, staged, onSubmit]);

  if (touchedFiles.length === 0) {
    return (
      <div className="px-4 py-3 text-xs text-text-faint">
        No staged comments.
      </div>
    );
  }

  return (
    <div className="flex flex-col">
      {/* Toolbar */}
      <div className="flex items-center justify-between px-4 py-1.5 border-b border-bg-border">
        <div className="flex items-center gap-2">
          <Checkbox
            checked={staged.size === touchedFiles.length}
            onCheckedChange={toggleAll}
            className="w-3.5 h-3.5"
          />
          <span className="text-[10px] text-text-faint">
            {staged.size}/{touchedFiles.length} staged
          </span>
        </div>
        <ToggleGroup
          type="single"
          value={viewMode}
          onValueChange={(v) => v && setViewMode(v as "flat" | "tree")}
          className="gap-0"
        >
          <ToggleGroupItem value="flat" className="h-6 w-6 p-0">
            <List className="w-3 h-3" />
          </ToggleGroupItem>
          <ToggleGroupItem value="tree" className="h-6 w-6 p-0">
            <FolderTree className="w-3 h-3" />
          </ToggleGroupItem>
        </ToggleGroup>
      </div>

      {/* File list */}
      <ScrollArea className="max-h-40">
        <div className="flex flex-col">
          {touchedFiles.map((file) => (
            <div
              key={file}
              className="flex items-center gap-2 px-4 py-1.5 hover:bg-bg-hover"
            >
              <Checkbox
                checked={staged.has(file)}
                onCheckedChange={() => toggleStaged(file)}
                className="w-3.5 h-3.5"
              />
              <FileText className="w-3 h-3 text-text-faint shrink-0" />
              <button
                className="text-xs font-mono text-text-muted truncate text-left hover:text-text-normal"
                onClick={() => onOpenFile?.(file)}
              >
                {viewMode === "flat" ? file : file.split("/").pop()}
              </button>
            </div>
          ))}
        </div>
      </ScrollArea>

      {/* Submit */}
      <div className="flex items-center justify-end px-4 py-2 border-t border-bg-border">
        <Button
          size="sm"
          className="h-7 px-3 text-xs bg-accent text-white hover:bg-accent-hover"
          disabled={staged.size === 0}
          onClick={handleSubmit}
        >
          <Send className="w-3 h-3 mr-1" />
          Submit to Claude
        </Button>
      </div>
    </div>
  );
}

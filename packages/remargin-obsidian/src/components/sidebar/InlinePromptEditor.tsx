import type { EditorView } from "@codemirror/view";
import { Check, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { ObsidianIcon } from "@/components/ui/ObsidianIcon";
import { createCommentEditor } from "@/editor/commentEditor";

function noop(): void {
  /* intentionally empty */
}

export interface InlinePromptEditorSaveArgs {
  /** Target `.remargin.yaml` for the write. */
  source: string;
  /** Final name (may be empty). */
  name: string;
  /** Final prompt body (may be empty). */
  prompt: string;
}

export interface InlinePromptEditorProps {
  /** Existing `.remargin.yaml` when editing; `null` when creating. */
  source: string | null;
  /**
   * Target folder. When creating, the parent writes to
   * `${folder}/.remargin.yaml`. When editing, the parent derives the
   * source's directory; this prop is passed for symmetry so the editor
   * doesn't have to compute it.
   */
  folder: string;
  initialName: string;
  initialBody: string;
  onSave: (args: InlinePromptEditorSaveArgs) => Promise<void>;
  onDelete?: (source: string) => Promise<void>;
  onCancel: () => void;
  /**
   * When set, the Save button is disabled and the tooltip explains
   * why (e.g. strict mode without a key). The editor stays open and
   * editable so the user can still copy the buffer.
   */
  saveDisabledReason?: string;
  /**
   * Vault folders the create-mode picker can offer. Ignored in edit
   * mode (the folder is locked to the existing `source`'s dirname).
   * Vault root is represented as an empty string in this list.
   */
  availableFolders?: string[];
}

/**
 * Inline editor for the folder-scoped `system_prompt:` block. Renders
 * inside a prompt group header in place of the group's body when the
 * user clicks the gear (edit) or "+ Configure" (create) affordance.
 *
 * Layout matches `ui_components.pen` frame `cTujj`: a NAME input, a
 * CM6 PROMPT editor with markdown highlighting, the target file scope
 * line, and Delete / Cancel / Save buttons.
 */
export function InlinePromptEditor({
  source,
  folder,
  initialName,
  initialBody,
  onSave,
  onDelete,
  onCancel,
  saveDisabledReason,
  availableFolders,
}: InlinePromptEditorProps) {
  const isCreate = source === null;
  const [name, setName] = useState(initialName);
  const [submitting, setSubmitting] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [chosenFolder, setChosenFolder] = useState(folder);
  const [folderQuery, setFolderQuery] = useState("");
  const [showFolderDropdown, setShowFolderDropdown] = useState(false);
  const folderPickerRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const submitRef = useRef<() => void>(noop);
  const cancelRef = useRef<() => void>(noop);
  // Captured once so the mount effect can seed CM6 without depending on
  // the prop directly; subsequent prop changes are intentionally
  // ignored (the user is editing the buffer at that point).
  const initialBodyRef = useRef(initialBody);

  const getBody = useCallback((): string => {
    return viewRef.current?.state.doc.toString() ?? "";
  }, []);

  const handleSubmit = useCallback(async () => {
    if (submitting || saveDisabledReason) return;
    const prompt = getBody();
    setSubmitting(true);
    setError(null);
    try {
      const dir = (source === null ? chosenFolder : folder).replace(/\/$/u, "");
      const target = source ?? (dir === "" ? ".remargin.yaml" : `${dir}/.remargin.yaml`);
      await onSave({ source: target, name, prompt });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }, [submitting, saveDisabledReason, source, folder, chosenFolder, name, onSave, getBody]);

  const handleDelete = useCallback(async () => {
    if (deleting || !onDelete || !source) return;
    const confirmed =
      typeof window !== "undefined" && typeof window.confirm === "function"
        ? window.confirm(
            `Delete prompt "${initialName || "default"}"? Files in this folder will fall back to the parent prompt (or Default).`
          )
        : true;
    if (!confirmed) return;
    setDeleting(true);
    setError(null);
    try {
      await onDelete(source);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeleting(false);
    }
  }, [deleting, onDelete, source, initialName]);

  submitRef.current = () => void handleSubmit();
  cancelRef.current = onCancel;

  // Mount the CM6 body editor on first render. The seed body is read
  // once from a ref so the effect's dep list stays empty without
  // tripping the exhaustive-deps lint.
  useEffect(() => {
    if (!editorRef.current) return undefined;
    const view = createCommentEditor({
      parent: editorRef.current,
      placeholder: "System prompt body... (markdown)",
      onSubmit: () => submitRef.current(),
      onCancel: () => cancelRef.current(),
    });
    const seed = initialBodyRef.current;
    if (seed) {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: seed },
      });
    }
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, []);

  const filteredFolders = useMemo(() => {
    const q = folderQuery.trim().toLowerCase();
    const list = availableFolders ?? [];
    if (!q) return list;
    return list.filter((f) => f.toLowerCase().includes(q));
  }, [availableFolders, folderQuery]);

  // Close the folder dropdown when the user clicks outside the picker.
  // mousedown (not click) so the close happens before any inner button's
  // onClick — otherwise picking from the list races with the outside-close.
  useEffect(() => {
    if (!showFolderDropdown) return undefined;
    const onDocumentMouseDown = (e: MouseEvent) => {
      if (!folderPickerRef.current) return;
      if (!folderPickerRef.current.contains(e.target as Node)) {
        setShowFolderDropdown(false);
      }
    };
    document.addEventListener("mousedown", onDocumentMouseDown);
    return () => document.removeEventListener("mousedown", onDocumentMouseDown);
  }, [showFolderDropdown]);

  const editTargetLabel = source ?? `${folder.replace(/\/$/u, "")}/.remargin.yaml`;
  const createTargetLabel = (() => {
    const dir = chosenFolder.replace(/\/$/u, "");
    return dir === "" ? ".remargin.yaml" : `${dir}/.remargin.yaml`;
  })();
  const saveLabel = isCreate ? "Create" : "Save";

  return (
    <div
      className="flex flex-col gap-2 px-4 py-3 border-t border-bg-border bg-bg-primary"
      data-testid="inline-prompt-editor"
    >
      <div className="flex items-center justify-between">
        <span className="text-[10px] uppercase tracking-wide text-text-faint">
          {isCreate ? "Create prompt" : "Editing prompt"}
        </span>
        <button
          type="button"
          className="flex items-center justify-center w-5 h-5 rounded-sm text-text-faint hover:text-text-normal hover:bg-bg-border"
          title="Cancel"
          onClick={onCancel}
        >
          <ObsidianIcon icon="x" size={12} />
        </button>
      </div>

      <label className="flex flex-col gap-1">
        <span className="text-[10px] uppercase tracking-wide text-text-faint">Name</span>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="(folder basename when empty)"
          className="w-full p-1.5 text-xs font-mono bg-bg-primary border border-bg-border rounded-sm text-text-normal placeholder:text-text-faint focus:outline-none focus:ring-1 focus:ring-accent"
        />
      </label>

      <label className="flex flex-col gap-1">
        <span className="text-[10px] uppercase tracking-wide text-text-faint">Prompt</span>
        <div
          ref={editorRef}
          className="min-h-[120px] border border-bg-border rounded-sm bg-bg-primary"
        />
      </label>

      {isCreate ? (
        <label className="flex flex-col gap-1">
          <span className="text-[10px] uppercase tracking-wide text-text-faint">Folder</span>
          <div ref={folderPickerRef} className="relative">
            <input
              type="text"
              value={folderQuery}
              onChange={(e) => setFolderQuery(e.target.value)}
              onFocus={() => setShowFolderDropdown(true)}
              placeholder={chosenFolder === "" ? "(vault root)" : chosenFolder}
              className="w-full p-1.5 text-xs font-mono bg-bg-primary border border-bg-border rounded-sm text-text-normal placeholder:text-text-faint focus:outline-none focus:ring-1 focus:ring-accent"
            />
            {showFolderDropdown && filteredFolders.length > 0 && (
              <div className="absolute z-10 mt-1 w-full max-h-40 overflow-y-auto bg-bg-secondary border border-bg-border rounded-sm shadow">
                {filteredFolders.map((f) => (
                  <button
                    key={f || "__root__"}
                    type="button"
                    className="w-full text-left px-2 py-1 text-xs font-mono text-text-normal hover:bg-bg-hover"
                    onClick={() => {
                      setChosenFolder(f);
                      setFolderQuery("");
                      setShowFolderDropdown(false);
                    }}
                  >
                    {f === "" ? "(vault root)" : f}
                  </button>
                ))}
              </div>
            )}
          </div>
          <span className="text-[9px] text-text-faint font-mono">
            Will write to: {createTargetLabel}
          </span>
        </label>
      ) : (
        <div className="flex items-center gap-1 text-[10px] text-text-faint font-mono">
          <span>{editTargetLabel}</span>
        </div>
      )}

      {error && (
        <div className="text-[10px] text-red-400 font-mono whitespace-pre-wrap break-words">
          {error}
        </div>
      )}

      <div className="flex items-center justify-between gap-2">
        <div>
          {!isCreate && onDelete && (
            <Button
              size="sm"
              variant="ghost"
              className="h-6 px-2 text-[10px] text-red-400 hover:text-red-300"
              disabled={deleting}
              onClick={handleDelete}
            >
              <Trash2 className="w-3 h-3 mr-1" />
              {deleting ? "Deleting..." : "Delete"}
            </Button>
          )}
        </div>
        <div className="flex items-center gap-1">
          <Button size="sm" variant="ghost" className="h-6 px-2 text-[10px]" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            size="sm"
            className="h-6 px-2 text-[10px] bg-accent text-white hover:bg-accent-hover"
            disabled={submitting || Boolean(saveDisabledReason)}
            title={saveDisabledReason}
            onClick={handleSubmit}
          >
            <Check className="w-3 h-3 mr-1" />
            {submitting ? "Saving..." : saveLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}

/** Groups sandboxed files by their resolved system prompt. */

import type { ResolvedSystemPrompt } from "@/backend/types";

export const DEFAULT_GROUP_KEY = "__default__";

/**
 * One Submit payload entry: the staged files of one resolved-prompt
 * group. The submit flow composes one prompt file + runner command per
 * entry and chains them in a single terminal launch.
 */
export interface StagedGroup {
  prompt: ResolvedSystemPrompt;
  files: string[];
}

/** One group of sandboxed files that resolved to the same system prompt. */
export interface PromptGroup {
  /** `null` for the Default group. */
  source: string | null;
  name: string;
  /** Owning folder, from `dirname(source)`; `(vault)` for Default. */
  scope: string;
  prompt: ResolvedSystemPrompt;
  files: string[];
  staged: string[];
  unstaged: string[];
  isDefault: boolean;
  hasError?: boolean;
  errorMessage?: string;
}

/** Strip trailing basename to derive the owning folder of a path. */
function dirOf(path: string): string {
  const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (idx <= 0) return "/";
  return path.slice(0, idx);
}

/**
 * Bucket sandboxed files by their resolved system prompt. Explicit
 * groups land first sorted by scope (lexicographic); a synthetic
 * `(error)` group lands above Default when one or more files failed
 * to resolve; the Default fallback group is always last when present.
 *
 * Files whose resolver result is missing (the network round-trip is
 * still in flight on a fresh refresh) land in a Default placeholder
 * so the UI never goes blank mid-fetch.
 */
export function buildPromptGroups(
  files: string[],
  prompts: Map<string, ResolvedSystemPrompt>,
  resolveErrors: Map<string, string>,
  staged: Set<string>
): PromptGroup[] {
  const byKey = new Map<string, PromptGroup>();
  const explicit: PromptGroup[] = [];
  let defaultGroup: PromptGroup | undefined;
  let errorGroup: PromptGroup | undefined;

  for (const file of files) {
    if (resolveErrors.has(file)) {
      if (!errorGroup) {
        errorGroup = makeErrorGroup(resolveErrors.get(file) ?? "resolve failed");
        byKey.set("__error__", errorGroup);
      }
      pushFile(errorGroup, file, staged);
      continue;
    }
    const resolved = prompts.get(file);
    if (!resolved) {
      if (!defaultGroup) {
        defaultGroup = makeDefaultPlaceholder();
        byKey.set(DEFAULT_GROUP_KEY, defaultGroup);
      }
      pushFile(defaultGroup, file, staged);
      continue;
    }
    if (resolved.is_default) {
      if (!defaultGroup) {
        defaultGroup = makeDefaultGroup(resolved);
        byKey.set(DEFAULT_GROUP_KEY, defaultGroup);
      } else {
        defaultGroup.prompt = resolved;
        defaultGroup.name = resolved.name;
        defaultGroup.isDefault = true;
      }
      pushFile(defaultGroup, file, staged);
      continue;
    }
    const key = resolved.source ?? `name:${resolved.name}`;
    let group = byKey.get(key);
    if (!group) {
      group = makeExplicitGroup(resolved);
      byKey.set(key, group);
      explicit.push(group);
    }
    pushFile(group, file, staged);
  }

  explicit.sort((a, b) => a.scope.localeCompare(b.scope));
  const out: PromptGroup[] = [...explicit];
  if (errorGroup) out.push(errorGroup);
  if (defaultGroup) out.push(defaultGroup);
  return out;
}

function pushFile(group: PromptGroup, file: string, staged: Set<string>) {
  group.files.push(file);
  if (staged.has(file)) group.staged.push(file);
  else group.unstaged.push(file);
}

function makeExplicitGroup(resolved: ResolvedSystemPrompt): PromptGroup {
  const source = resolved.source ?? null;
  return {
    source,
    name: resolved.name,
    scope: source ? dirOf(source) : "(unknown)",
    prompt: resolved,
    files: [],
    staged: [],
    unstaged: [],
    isDefault: false,
  };
}

function makeDefaultGroup(resolved: ResolvedSystemPrompt): PromptGroup {
  return {
    source: null,
    name: resolved.name,
    scope: "(vault)",
    prompt: resolved,
    files: [],
    staged: [],
    unstaged: [],
    isDefault: true,
  };
}

function makeDefaultPlaceholder(): PromptGroup {
  return {
    source: null,
    name: "default",
    scope: "(vault)",
    prompt: { is_default: true, name: "default", prompt: "", source: null },
    files: [],
    staged: [],
    unstaged: [],
    isDefault: true,
  };
}

function makeErrorGroup(message: string): PromptGroup {
  return {
    source: null,
    name: "(error)",
    scope: "resolve failed",
    prompt: { is_default: true, name: "(error)", prompt: "", source: null },
    files: [],
    staged: [],
    unstaged: [],
    isDefault: false,
    hasError: true,
    errorMessage: message,
  };
}

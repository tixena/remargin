import type {
  Acknowledgment,
  AuthorType,
  Comment,
  OnDiskComment,
  ReactionEntry,
} from "@/generated";

const LEGACY_REACTION_TS = "1970-01-01T00:00:00+00:00";

function normalizeAcks(
  raw: Array<string | { author: string; ts: string }> | undefined
): Acknowledgment[] {
  if (!raw) return [];
  return raw.flatMap((entry) => {
    if (typeof entry === "string") {
      // Wire shape: "author@rfc3339-ts"
      const at = entry.indexOf("@");
      if (at < 0) return [];
      return [{ author: entry.slice(0, at), ts: new Date(entry.slice(at + 1)) }];
    }
    return [{ author: entry.author, ts: new Date(entry.ts) }];
  });
}

function normalizeReactions(
  raw: Record<string, Array<string | { author: string; ts: string }>> | undefined
): Record<string, ReactionEntry[]> {
  if (!raw) return {};
  const out: Record<string, ReactionEntry[]> = {};
  for (const [emoji, items] of Object.entries(raw)) {
    out[emoji] = items.map((item) =>
      typeof item === "string"
        ? { author: item, ts: new Date(LEGACY_REACTION_TS) }
        : { author: item.author, ts: new Date(item.ts) }
    );
  }
  return out;
}

export interface ParsedBlock {
  startLine: number;
  endLine: number;
  startOffset: number;
  endOffset: number;
  fenceDepth: number;
  comment: Partial<Comment>;
  raw: string;
  valid: boolean;
  error?: string;
  warning?: string;
}

const enum State {
  Body,
  YamlHeader,
  Content,
}

// The YAML header parses into a partial OnDiskComment — the field names
// (e.g. `type`, `reply-to`) come straight from the generated TS
// interface, which mirrors `OnDiskComment`'s serde renames in
// `crates/remargin-core/src/on_disk_comment.rs`. The caller maps the
// wire shape onto the in-memory `Comment` (e.g. `type` →
// `author_type`).
type YamlFields = Partial<OnDiskComment> & {
  // The legacy on-disk `ack` shape allowed `{author, ts}` objects;
  // current writes emit `"author@ts"` strings. Tolerate both during the
  // YAML scan; the construction pass collapses the variation.
  ack?: Array<string | { author: string; ts: string }>;
  reactions?: Record<string, Array<string | { author: string; ts: string }>>;
};

function parseSimpleYaml(lines: string[]): YamlFields {
  const bag: Record<string, unknown> = {};
  let currentKey = "";
  let currentList: string[] | null = null;

  for (const line of lines) {
    const trimmed = line.trimEnd();

    if (trimmed.startsWith("  - ") || trimmed.startsWith("    - ")) {
      const value = trimmed.replace(/^\s*-\s*/, "");
      if (currentList) {
        currentList.push(value);
      }
      continue;
    }

    if (currentList && currentKey) {
      bag[currentKey] = currentList;
      currentList = null;
    }

    // `[\w-]+` so YAML keys with hyphens (e.g. `reply-to`) parse. The
    // Rust writer uses kebab-case for `reply-to`; without the hyphen
    // here the line is silently dropped and threading breaks.
    const match = trimmed.match(/^([\w-]+):\s*(.*)/);
    if (!match) continue;

    const [, key, rawValue] = match;
    const value = rawValue.trim();

    if (value === "" || value === "[]") {
      currentKey = key;
      currentList = [];
      continue;
    }

    if (value.startsWith("[") && value.endsWith("]")) {
      const inner = value.slice(1, -1);
      bag[key] = inner ? inner.split(",").map((s) => s.trim().replace(/^["']|["']$/g, "")) : [];
      currentKey = "";
      continue;
    }

    bag[key] = value.replace(/^["']|["']$/g, "");
    currentKey = key;
    currentList = null;
  }

  if (currentList && currentKey) {
    bag[currentKey] = currentList;
  }

  return bag as YamlFields;
}

export function parseRemarginBlocks(text: string): ParsedBlock[] {
  const results: ParsedBlock[] = [];
  const lines = text.split("\n");
  let state = State.Body;
  let fenceDepth = 0;
  let blockStartLine = 0;
  let blockStartOffset = 0;
  let yamlLines: string[] = [];
  let contentLines: string[] = [];
  let yamlClosed = false;
  let offset = 0;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const lineStart = offset;
    offset += line.length + 1; // +1 for \n

    switch (state) {
      case State.Body: {
        const match = line.match(/^(`{3,})remargin\s*$/);
        if (match) {
          fenceDepth = match[1].length;
          blockStartLine = i + 1; // 1-indexed
          blockStartOffset = lineStart;
          yamlLines = [];
          contentLines = [];
          yamlClosed = false;
          state = State.YamlHeader;
        }
        break;
      }

      case State.YamlHeader: {
        if (line.trim() === "---") {
          if (yamlClosed) {
            state = State.Content;
          } else {
            yamlClosed = true;
          }
        } else if (yamlClosed) {
          yamlLines.push(line);
        } else {
          // Malformed before first --- — recover by treating as body.
          state = State.Body;
        }
        break;
      }

      case State.Content: {
        const closingFence = "`".repeat(fenceDepth);
        if (line.trim() === closingFence) {
          const yaml = parseSimpleYaml(yamlLines);
          const content = contentLines.join("\n");
          const block: ParsedBlock = {
            startLine: blockStartLine,
            endLine: i + 1,
            startOffset: blockStartOffset,
            endOffset: offset,
            fenceDepth,
            raw: lines.slice(blockStartLine - 1, i + 1).join("\n"),
            comment: {
              id: yaml.id,
              author: yaml.author,
              // OnDiskComment renames `author_type` → `type` for the
              // wire form; map back to the in-memory field here.
              author_type: yaml.type as AuthorType | undefined,
              ts: yaml.ts ? new Date(yaml.ts) : undefined,
              content,
              // OnDiskComment renames `reply_to` → `reply-to`; map back.
              reply_to: yaml["reply-to"],
              thread: yaml.thread,
              to: yaml.to ?? [],
              ack: normalizeAcks(yaml.ack),
              reactions: normalizeReactions(yaml.reactions),
              attachments: yaml.attachments ?? [],
              checksum: yaml.checksum,
              signature: yaml.signature,
              line: blockStartLine,
              sl: blockStartLine,
              el: i + 1,
            },
            valid: true,
          };

          if (!yaml.id) {
            block.valid = false;
            block.error = "missing id";
          }

          results.push(block);
          state = State.Body;
        } else {
          contentLines.push(line);
        }
        break;
      }
    }
  }

  if (state !== State.Body) {
    results.push({
      startLine: blockStartLine,
      endLine: lines.length,
      startOffset: blockStartOffset,
      endOffset: text.length,
      fenceDepth,
      raw: lines.slice(blockStartLine - 1).join("\n"),
      comment: {},
      valid: false,
      error: "unclosed block",
    });
  }

  return results;
}

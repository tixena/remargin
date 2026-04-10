import type { AuthorType, Comment } from "@/generated";

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

interface YamlFields {
  id?: string;
  author?: string;
  author_type?: string;
  ts?: string;
  reply_to?: string;
  thread?: string;
  to?: string[];
  ack?: Array<{ author: string; ts: string }>;
  reactions?: Record<string, string[]>;
  attachments?: string[];
  checksum?: string;
  signature?: string;
}

function parseSimpleYaml(lines: string[]): YamlFields {
  const bag: Record<string, unknown> = {};
  let currentKey = "";
  let currentList: string[] | null = null;

  for (const line of lines) {
    const trimmed = line.trimEnd();

    // List item continuation
    if (trimmed.startsWith("  - ") || trimmed.startsWith("    - ")) {
      const value = trimmed.replace(/^\s*-\s*/, "");
      if (currentList) {
        currentList.push(value);
      }
      continue;
    }

    // Flush previous list
    if (currentList && currentKey) {
      bag[currentKey] = currentList;
      currentList = null;
    }

    // Key: value pair
    const match = trimmed.match(/^(\w+):\s*(.*)/);
    if (!match) continue;

    const [, key, rawValue] = match;
    const value = rawValue.trim();

    if (value === "" || value === "[]") {
      // Start of list or empty
      currentKey = key;
      currentList = [];
      continue;
    }

    // Inline list: [a, b, c]
    if (value.startsWith("[") && value.endsWith("]")) {
      const inner = value.slice(1, -1);
      bag[key] = inner ? inner.split(",").map((s) => s.trim().replace(/^["']|["']$/g, "")) : [];
      currentKey = "";
      continue;
    }

    // Simple scalar
    bag[key] = value.replace(/^["']|["']$/g, "");
    currentKey = key;
    currentList = null;
  }

  // Flush trailing list
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
            // Second --- closes YAML, transition to content
            state = State.Content;
          } else {
            yamlClosed = true;
          }
        } else if (yamlClosed) {
          // After first ---, accumulate YAML
          yamlLines.push(line);
        } else {
          // Before first ---, this is malformed
          // Try to recover — treat as body text
          state = State.Body;
        }
        break;
      }

      case State.Content: {
        const closingFence = "`".repeat(fenceDepth);
        if (line.trim() === closingFence) {
          // Block complete
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
              author_type: yaml.author_type as AuthorType | undefined,
              ts: yaml.ts,
              content,
              reply_to: yaml.reply_to,
              thread: yaml.thread,
              to: yaml.to ?? [],
              ack: yaml.ack ?? [],
              reactions: yaml.reactions ?? {},
              attachments: yaml.attachments ?? [],
              checksum: yaml.checksum,
              signature: yaml.signature,
              line: blockStartLine,
              fence_depth: fenceDepth,
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

  // Handle unclosed block
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

import { MarkdownPostProcessorContext } from "obsidian";

/**
 * Post-processor for reading mode that replaces remargin fenced code
 * blocks with styled comment widgets.
 *
 * Registered via `this.registerMarkdownPostProcessor()` in main.ts.
 */
export function remarginPostProcessor(
  el: HTMLElement,
  _ctx: MarkdownPostProcessorContext
): void {
  const codeBlocks = el.querySelectorAll<HTMLElement>(
    'pre > code.language-remargin'
  );

  for (const code of codeBlocks) {
    const pre = code.parentElement;
    if (!pre) continue;

    const raw = code.textContent ?? "";
    const lines = raw.split("\n");

    // Parse YAML header between --- markers
    let yamlStart = -1;
    let yamlEnd = -1;
    for (let i = 0; i < lines.length; i++) {
      if (lines[i].trim() === "---") {
        if (yamlStart === -1) {
          yamlStart = i;
        } else {
          yamlEnd = i;
          break;
        }
      }
    }

    if (yamlStart === -1 || yamlEnd === -1) continue;

    // Extract fields from YAML
    const yamlLines = lines.slice(yamlStart + 1, yamlEnd);
    const fields: Record<string, string> = {};
    for (const line of yamlLines) {
      const match = line.match(/^(\w+):\s*(.*)/);
      if (match) {
        fields[match[1]] = match[2].trim().replace(/^["']|["']$/g, "");
      }
    }

    const content = lines.slice(yamlEnd + 1).join("\n").trim();

    // Build widget
    const widget = document.createElement("div");
    widget.className = "remargin-reading-widget";

    const header = document.createElement("div");
    header.className = "remargin-reading-header";

    const badge = document.createElement("span");
    badge.className = `remargin-badge remargin-badge-${
      fields.author_type?.toLowerCase() === "agent" ? "agent" : "human"
    }`;
    badge.textContent =
      fields.author_type?.toLowerCase() === "agent" ? "AI" : "H";
    header.appendChild(badge);

    const author = document.createElement("span");
    author.className = "remargin-reading-author";
    author.textContent = fields.author ?? "unknown";
    header.appendChild(author);

    if (fields.ts) {
      const time = document.createElement("span");
      time.className = "remargin-reading-time";
      time.textContent = fields.ts;
      header.appendChild(time);
    }

    widget.appendChild(header);

    if (content) {
      const body = document.createElement("div");
      body.className = "remargin-reading-content";
      body.textContent = content;
      widget.appendChild(body);
    }

    pre.replaceWith(widget);
  }
}

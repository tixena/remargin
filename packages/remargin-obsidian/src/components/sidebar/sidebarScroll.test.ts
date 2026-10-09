/**
 * Source-text checks on the sidebar's scroll container: the `min-h-0` that lets `flex-1` cap
 * the ScrollArea, and scrollbar colour classes that resolve to a real palette token.
 */

import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

// Asserted against the source text: the bug lives in the class list, and jsdom computes no box
// sizes to observe it through a rendered tree.
const here = dirname(fileURLToPath(import.meta.url));
const shellSource = readFileSync(join(here, "SidebarShell.tsx"), "utf8");
const scrollAreaSource = readFileSync(join(here, "..", "ui", "scroll-area.tsx"), "utf8");

/** The class list of the `<ScrollArea>` element in `SidebarShell`. */
function scrollAreaClasses(): string {
  const match = shellSource.match(/<ScrollArea className="([^"]+)"/);
  assert.ok(match, "SidebarShell must render a <ScrollArea> with a className");
  return match[1];
}

describe("sidebar scroll container", () => {
  it("sets min-h-0 so flex-1 can actually cap its height", () => {
    assert.match(
      scrollAreaClasses(),
      /\bmin-h-0\b/,
      "without min-h-0 the panel grows to its content and never scrolls itself"
    );
  });

  it("keeps min-w-0 for the same reason on the horizontal axis", () => {
    assert.match(scrollAreaClasses(), /\bmin-w-0\b/);
  });

  it("still stretches to the available space", () => {
    assert.match(scrollAreaClasses(), /\bflex-1\b/);
  });

  it("lives inside a full-height flex column, which is what flex-1 measures", () => {
    assert.ok(
      shellSource.includes('className="flex flex-col h-full min-w-0 bg-bg-primary"'),
      "the shell root must stay a full-height flex column for min-h-0 to matter"
    );
  });

  it("renders an always-visible scrollbar, so a capped height shows a thumb", () => {
    assert.match(scrollAreaSource, /type="always"/);
  });
});

// Radix paints its own thumb, and Tailwind emits no rule for an unknown colour, so a colour
// class outside this project's palette leaves the scrollbar invisible without any error.

/** Colour tokens defined in `tailwind.config.ts`, e.g. `bg-border`. */
function paletteTokens(): Set<string> {
  const config = readFileSync(join(here, "..", "..", "..", "tailwind.config.ts"), "utf8");
  const block = config.match(/colors:\s*\{([\s\S]*?)\n\s{6}\}/);
  assert.ok(block, "tailwind.config.ts must declare a colors block");
  const tokens = new Set<string>();
  for (const [, quoted, bare] of block[1].matchAll(/^\s*(?:"([^"]+)"|([\w-]+)):/gm)) {
    tokens.add(quoted ?? bare);
  }
  return tokens;
}

/** Colour utilities Tailwind ships regardless of the configured palette. */
const BUILTIN_COLORS = new Set(["transparent", "current", "inherit", "black", "white"]);

describe("scroll-area colour classes", () => {
  it("paints the thumb with a colour the palette actually defines", () => {
    const thumb = scrollAreaSource.match(/ScrollAreaThumb className="([^"]+)"/);
    assert.ok(thumb, "the scrollbar thumb must carry a className");
    const background = thumb[1].split(/\s+/).find((cls) => cls.startsWith("bg-"));
    assert.ok(background, "the thumb needs a background colour to be visible");
    const token = background.replace(/^bg-/, "");
    assert.ok(
      paletteTokens().has(token) || BUILTIN_COLORS.has(token),
      `thumb background "${background}" resolves to no colour: ` +
        `"${token}" is not in the palette, so the scrollbar renders invisible`
    );
  });

  it("names no background colour the palette lacks", () => {
    const palette = paletteTokens();
    // Whole classes in `className="..."` only: raw text would find `bg-border` in `bg-bg-border`.
    const unresolved: string[] = [];
    for (const [, literal] of scrollAreaSource.matchAll(/className="([^"]*)"/g)) {
      for (const cls of literal.split(/\s+/)) {
        const token = cls.replace(/^!/, "").match(/^bg-(.+)$/)?.[1];
        // `bg-` also prefixes non-colour utilities (bg-clip, bg-cover, ...).
        if (
          !token ||
          /^(?:clip|cover|contain|center|repeat|no-repeat|gradient|origin|\[)/.test(token)
        )
          continue;
        if (palette.has(token) || BUILTIN_COLORS.has(token)) continue;
        unresolved.push(cls);
      }
    }
    assert.deepEqual(
      unresolved,
      [],
      "Tailwind emits no rule for an unknown colour, so these paint nothing: " +
        unresolved.join(", ")
    );
  });
});

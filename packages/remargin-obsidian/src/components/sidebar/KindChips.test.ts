import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { KindChips } from "./KindChips.tsx";

describe("KindChips", () => {
  it("renders one chip per kind, in stored order", () => {
    const html = renderToStaticMarkup(
      createElement(KindChips, { kinds: ["decision-done", "action item"] })
    );
    const first = html.indexOf('aria-label="Kind: decision-done"');
    const second = html.indexOf('aria-label="Kind: action item"');
    assert.ok(first > -1 && second > first, html);
    assert.ok(html.includes(">decision-done</span>"), html);
  });

  it("renders nothing when the comment has no kind", () => {
    for (const kinds of [[], undefined]) {
      assert.equal(renderToStaticMarkup(createElement(KindChips, { kinds })), "");
    }
  });

  it("is a rounded chip at Obsidian's input height", () => {
    const html = renderToStaticMarkup(createElement(KindChips, { kinds: ["decision-item"] }));
    assert.match(
      html,
      /class="[^"]*rounded-full[^"]*h-\[var\(--input-height\)\]|class="[^"]*h-\[var\(--input-height\)\][^"]*rounded-full/
    );
  });
});

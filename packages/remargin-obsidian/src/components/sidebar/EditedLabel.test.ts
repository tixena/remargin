import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { EditedLabel } from "./EditedLabel.tsx";

describe("EditedLabel", () => {
  it("reads `edited` with the relative time", () => {
    const editedAt = new Date(Date.now() - 2 * 60 * 60 * 1000);
    const html = renderToStaticMarkup(createElement(EditedLabel, { editedAt }));
    assert.ok(html.includes("edited 2h</span>"), html);
  });

  it("carries the exact edit time in its accessible label and hover title", () => {
    const html = renderToStaticMarkup(
      createElement(EditedLabel, { editedAt: new Date("2026-09-26T16:39:09Z") })
    );
    assert.match(html, /aria-label="Edited [^"]*2026[^"]*"/);
    assert.match(html, /title="Edited [^"]*2026[^"]*"/);
  });

  it("takes Obsidian's input height so it lines up with the Reply button", () => {
    const html = renderToStaticMarkup(createElement(EditedLabel, { editedAt: new Date() }));
    assert.ok(html.includes("h-[var(--input-height)]"), html);
  });
});

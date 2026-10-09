/** Static-markup tests for the section header's structure. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { Mail } from "lucide-react";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Collapsible } from "../ui/collapsible.tsx";
import { SectionHeader } from "./SectionHeader.tsx";
import { ViewToggle } from "./ViewToggle.tsx";

// SSR-only render — the trigger needs a Radix Collapsible root above it.

const noop = (): void => {
  /* test-only no-op */
};

function render(open: boolean, badgeVariant: "default" | "warning" = "default"): string {
  return renderToStaticMarkup(
    createElement(
      Collapsible,
      { open },
      createElement(SectionHeader, {
        icon: Mail,
        title: "Inbox",
        badge: 3,
        badgeVariant,
        open,
        actions: createElement(ViewToggle, { value: "flat", onChange: noop }),
      })
    )
  );
}

/** The HTML content model forbids interactive content inside <button>. */
function assertNoNestedButtons(html: string): void {
  let depth = 0;
  for (const match of html.matchAll(/<(\/?)button\b/g)) {
    if (match[1] === "/") {
      depth -= 1;
    } else {
      assert.equal(depth, 0, `<button> nested inside <button> at index ${match.index}: ${html}`);
      depth += 1;
    }
  }
  assert.equal(depth, 0, `unbalanced <button> tags: ${html}`);
}

describe("SectionHeader — actions render outside the trigger button", () => {
  it("no <button> has a <button> descendant", () => {
    const html = render(true);
    const buttonCount = [...html.matchAll(/<button\b/g)].length;
    assert.equal(buttonCount, 3, `expected trigger + 2 toggle buttons, got: ${html}`);
    assertNoNestedButtons(html);
  });

  it("no click-shield wrapper remains", () => {
    const html = render(true);
    assert.ok(!html.includes('role="presentation"'), `expected no shield wrapper, got: ${html}`);
  });
});

describe("SectionHeader — one chrome for every section", () => {
  it("always renders the L1 chrome with the open state on the row", () => {
    const html = render(false);
    assert.ok(html.includes('class="rmg-l1-head" data-open="false"'), html);
    assert.ok(html.includes('class="rmg-l1-head__trigger"'), html);
  });

  it("renders a single chevron icon and leaves rotation to the CSS", () => {
    for (const open of [true, false]) {
      const html = render(open);
      assert.ok(html.includes("lucide-chevron-down"), html);
      assert.ok(!html.includes("lucide-chevron-right"), html);
    }
  });

  it("marks the warning badge with its modifier class", () => {
    assert.ok(render(true, "warning").includes("rmg-l1-head__badge rmg-l1-head__badge--warning"));
    assert.ok(!render(true).includes("rmg-l1-head__badge--warning"));
  });
});

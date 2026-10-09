/** Tests for the plugin's `remargin:focus` event bus. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import RemarginPlugin, { type RemarginFocusDetail } from "./main.ts";

/**
 * A freshly instantiated plugin with only the focus-side pieces of `onload` populated: no
 * settings tab and no workspace event registrations.
 */
function makeFocusReadyPlugin(): RemarginPlugin {
  const plugin = new RemarginPlugin({} as never, {} as never);
  plugin.focusEvents = new EventTarget();
  return plugin;
}

describe("RemarginPlugin.focusComment", () => {
  it("dispatches remargin:focus with the comment id and file", () => {
    const plugin = makeFocusReadyPlugin();
    const captured: RemarginFocusDetail[] = [];
    plugin.focusEvents.addEventListener("remargin:focus", (event) => {
      const detail = (event as CustomEvent<RemarginFocusDetail>).detail;
      if (detail) captured.push(detail);
    });
    plugin.focusComment("c1", "notes/file.md");
    assert.deepStrictEqual(captured, [{ commentId: "c1", file: "notes/file.md" }]);
  });

  // Neither throws nor emits a console warning.
  it("is a silent no-op when no subscriber is attached", () => {
    const plugin = makeFocusReadyPlugin();
    const originalWarn = console.warn;
    let warnCalls = 0;
    console.warn = () => {
      warnCalls += 1;
    };
    try {
      assert.doesNotThrow(() => plugin.focusComment("x", "y.md"));
    } finally {
      console.warn = originalWarn;
    }
    assert.equal(warnCalls, 0, "expected no console.warn call");
  });

  // The file-switch call must precede the focus call; the DOM side lives in `focusCard.test.ts`.
  it("subscribers see file-switch and focus calls in dispatch order", () => {
    const plugin = makeFocusReadyPlugin();
    const calls: string[] = [];
    const setFilter = (file: string) => {
      calls.push(`setFilter:${file}`);
    };
    const focusCard = (id: string) => {
      calls.push(`focus:${id}`);
    };
    plugin.focusEvents.addEventListener("remargin:focus", (event) => {
      const detail = (event as CustomEvent<RemarginFocusDetail>).detail;
      if (!detail) return;
      // As SidebarShell does: switch the filter first when the file differs, then focus.
      const activeFile = "current.md";
      if (detail.file !== activeFile) setFilter(detail.file);
      focusCard(detail.commentId);
    });
    plugin.focusComment("c1", "other.md");
    assert.deepStrictEqual(calls, ["setFilter:other.md", "focus:c1"]);
  });

  it("subscribers skip setFilter when the event targets the active file", () => {
    const plugin = makeFocusReadyPlugin();
    const calls: string[] = [];
    const setFilter = (file: string) => {
      calls.push(`setFilter:${file}`);
    };
    const focusCard = (id: string) => {
      calls.push(`focus:${id}`);
    };
    plugin.focusEvents.addEventListener("remargin:focus", (event) => {
      const detail = (event as CustomEvent<RemarginFocusDetail>).detail;
      if (!detail) return;
      const activeFile = "current.md";
      if (detail.file !== activeFile) setFilter(detail.file);
      focusCard(detail.commentId);
    });
    plugin.focusComment("c1", "current.md");
    assert.deepStrictEqual(calls, ["focus:c1"]);
  });

  it("removeEventListener detaches the subscriber", () => {
    const plugin = makeFocusReadyPlugin();
    let calls = 0;
    const handler = () => {
      calls += 1;
    };
    plugin.focusEvents.addEventListener("remargin:focus", handler);
    plugin.focusComment("c1", "f.md");
    plugin.focusEvents.removeEventListener("remargin:focus", handler);
    plugin.focusComment("c2", "f.md");
    assert.equal(calls, 1);
  });
});

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import RemarginPlugin, { type RemarginFocusDetail } from "./main.ts";

/**
 * Build a freshly-instantiated plugin with the foundation pieces from
 * `onload` populated, but without spinning up the full activation
 * pipeline (no `addSettingTab`, no workspace event registrations). The
 * focus-event surface is the only thing T36 AC tests #0/#10/#11 care
 * about.
 */
function makeFocusReadyPlugin(): RemarginPlugin {
  const plugin = new RemarginPlugin({} as never, {} as never);
  // Match what `onload` would set up — just the focus-side bits.
  plugin.focusEvents = new EventTarget();
  return plugin;
}

describe("RemarginPlugin.focusComment", () => {
  // Test #0 (T36 spec): focusComment dispatches `remargin:focus` on
  // `plugin.focusEvents` with the right detail payload.
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

  // Test #11 (T36 spec): with no subscriber attached, focusComment is
  // a silent no-op — neither throws nor emits a console warning.
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

  // Test #10 part 1 (T36 spec, ordering portion that does not require
  // a real DOM): a subscriber that switches the filter on its first
  // call, then focuses the card, demonstrates the spec's contract:
  // the file-switch invocation precedes the focus invocation. The DOM
  // side of the contract is covered by `focusCard.test.ts`.
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
      // Mirrors SidebarShell's behaviour: switch the filter first when
      // the file differs from the current filter, then focus.
      const activeFile = "current.md";
      if (detail.file !== activeFile) setFilter(detail.file);
      focusCard(detail.commentId);
    });
    plugin.focusComment("c1", "other.md");
    assert.deepStrictEqual(calls, ["setFilter:other.md", "focus:c1"]);
  });

  // Companion to test #10: when the event names the active file, the
  // listener does NOT call `setFilter` first.
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

  // Subscribe/unsubscribe symmetry: removing the listener stops
  // further dispatches from firing, matching the React unmount path.
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

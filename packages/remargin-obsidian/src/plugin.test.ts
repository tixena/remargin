import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import RemarginPlugin from "./main.ts";
import { CollapseState } from "./state/collapseState.ts";

/**
 * Build the smallest `App` shape the plugin's `onload` actually
 * touches. The test asserts the T36 foundation pieces (`collapseState`,
 * `focusEvents`) are populated after `onload`, so anything onload
 * touches before/after that point must not throw.
 */
function makeApp(): unknown {
  const noopRef = {};
  const noop = () => {
    /* test-only stub */
  };
  return {
    vault: { adapter: { basePath: "/tmp/test-vault" } },
    workspace: {
      getActiveViewOfType: () => null,
      getLeavesOfType: () => [],
      getRightLeaf: () => null,
      getLeftLeaf: () => null,
      on: () => noopRef,
      off: noop,
      offref: noop,
      onLayoutReady: noop,
      revealLeaf: noop,
    },
  };
}

function makeManifest(): unknown {
  return { version: "0.0.0-test", id: "remargin", name: "Remargin" };
}

describe("RemarginPlugin onload (T36 foundation)", () => {
  // Test #12 (T36 spec): onload creates collapseState + focusEvents.
  it("creates plugin.collapseState and plugin.focusEvents", async () => {
    const plugin = new RemarginPlugin(makeApp() as never, makeManifest() as never);
    // Disable the update probe so onload's tail does not spawn the CLI
    // in a sandboxed test environment.
    plugin.settings = { ...plugin.settings, checkForUpdates: false };
    // `loadData` is the persistence shim from the stubbed Plugin base.
    // Returning a populated object steers `loadSettings` away from its
    // first-run CLI probe (which would spawn a process otherwise).
    Object.assign(plugin, {
      loadData: async () => ({ ...plugin.settings, checkForUpdates: false }),
      saveData: async () => {
        /* test-only no-op persistence */
      },
    });

    await plugin.onload();

    assert.ok(
      plugin.collapseState instanceof CollapseState,
      "expected plugin.collapseState to be a CollapseState"
    );
    assert.ok(
      plugin.focusEvents instanceof EventTarget,
      "expected plugin.focusEvents to be an EventTarget"
    );
  });
});

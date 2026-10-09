/** Tests that `onload` creates the plugin's collapse state and focus bus. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import RemarginPlugin from "./main.ts";
import { CollapseState } from "./state/collapseState.ts";

/** The smallest `App` shape the plugin's `onload` touches. */
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

describe("RemarginPlugin onload", () => {
  it("creates plugin.collapseState and plugin.focusEvents", async () => {
    const plugin = new RemarginPlugin(makeApp() as never, makeManifest() as never);
    // The update probe is disabled so `onload` does not spawn the CLI.
    plugin.settings = { ...plugin.settings, checkForUpdates: false };
    // A populated object steers `loadSettings` away from its first-run CLI probe.
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

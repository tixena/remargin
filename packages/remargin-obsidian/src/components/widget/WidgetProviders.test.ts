/** Tests that `WidgetProviders` supplies its three contexts and adds no markup. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement, useContext } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { RemarginBackend } from "../../backend/index.ts";
import { BackendContext } from "../../hooks/useBackend.ts";
import { PluginContext } from "../../hooks/usePlugin.ts";
import { PortalContainerContext } from "../../hooks/usePortalContainer.ts";
import type RemarginPlugin from "../../main.ts";
import { WidgetProviders } from "./WidgetProviders.tsx";

const backendStub = {} as unknown as RemarginBackend;
const pluginStub = { backend: backendStub } as unknown as RemarginPlugin;

/** What the probe component read from the three contexts. */
interface Captured {
  plugin: RemarginPlugin | null;
  backend: RemarginBackend | null;
  portal: HTMLElement | null;
}

function makeProbe(captured: Captured) {
  return function Probe() {
    captured.plugin = useContext(PluginContext);
    captured.backend = useContext(BackendContext);
    captured.portal = useContext(PortalContainerContext);
    return null;
  };
}

describe("WidgetProviders", () => {
  it("populates BackendContext, PluginContext, and PortalContainerContext", () => {
    const captured: Captured = { plugin: null, backend: null, portal: null };
    const portalStub = { __tag: "portal-host" } as unknown as HTMLElement;
    const Probe = makeProbe(captured);

    renderToStaticMarkup(
      createElement(
        WidgetProviders,
        { plugin: pluginStub, portalContainer: portalStub },
        createElement(Probe)
      )
    );

    assert.equal(captured.plugin, pluginStub, "PluginContext must carry the plugin stub");
    assert.equal(captured.backend, backendStub, "BackendContext must carry plugin.backend");
    assert.equal(captured.portal, portalStub, "PortalContainerContext must carry the host");
  });

  // An extra wrapper element would silently change layout.
  it("renders no DOM of its own — children's markup is the entire output", () => {
    const portalStub = {} as unknown as HTMLElement;
    const html = renderToStaticMarkup(
      createElement(
        WidgetProviders,
        { plugin: pluginStub, portalContainer: portalStub },
        createElement("span", { className: "probe-leaf" }, "leaf")
      )
    );
    assert.equal(
      html,
      '<span class="probe-leaf">leaf</span>',
      "WidgetProviders must be transparent in the DOM"
    );
  });
});

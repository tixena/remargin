/** Static-markup tests for the ack label's arrow and color rules. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { Participant, RemarginBackend } from "../../backend/index.ts";
import { BackendContext } from "../../hooks/useBackend.ts";
import { __resetParticipantsCacheForTests } from "../../hooks/useParticipants.ts";
import { PluginContext } from "../../hooks/usePlugin.ts";
import type RemarginPlugin from "../../main.ts";
import { DEFAULT_SETTINGS } from "../../types.ts";
import { AckToggle } from "./AckToggle.tsx";

const pluginStub = { settings: DEFAULT_SETTINGS } as unknown as RemarginPlugin;
const backendStub = {
  registryShow: (): Promise<Participant[]> => Promise.resolve([]),
} as unknown as RemarginBackend;

function render(props: { ack: string[]; me?: string | null; toTargets?: string[] }): string {
  __resetParticipantsCacheForTests();
  return renderToStaticMarkup(
    createElement(
      PluginContext.Provider,
      { value: pluginStub },
      createElement(
        BackendContext.Provider,
        { value: backendStub },
        createElement(AckToggle, props)
      )
    )
  );
}

describe("AckToggle", () => {
  it("renders a non-interactive span (not a button)", () => {
    const html = render({ ack: [], me: "eduardo" });
    assert.ok(html.startsWith("<span"), `expected <span>, got: ${html.slice(0, 80)}`);
    assert.ok(!html.includes("<button"), "expected no <button> wrapping the label");
    assert.ok(!html.includes("onClick"), "expected no onClick handler in static markup");
  });

  it("shows 'unacked' label when nobody has acked", () => {
    const html = render({ ack: [], me: "eduardo" });
    assert.ok(html.includes("unacked"), `expected 'unacked' label, got: ${html}`);
  });

  it("shows 'acked' label with count when only others acked", () => {
    const html = render({ ack: ["alice", "bob"], me: "eduardo" });
    assert.ok(html.includes("acked"));
    assert.ok(html.includes(">2<"), `expected count badge of 2, got: ${html}`);
  });

  it("shows 'acked' label when the current identity is in the list", () => {
    const html = render({ ack: ["eduardo"], me: "eduardo" });
    assert.ok(html.includes("acked"));
    assert.ok(html.includes(">1<"));
  });

  it("surfaces the roster in the tooltip when there are acks", () => {
    const html = render({ ack: ["alice"], me: "eduardo" });
    assert.ok(html.includes('title="acked by alice"'), `expected roster tooltip, got: ${html}`);
  });

  it("falls back to a neutral tooltip when the ack list is empty", () => {
    const html = render({ ack: [], me: "eduardo" });
    assert.ok(html.includes('title="No acknowledgments yet"'), `got: ${html}`);
  });

  // The four `ackVisualFor` rules, pinned at the DOM-class level.
  it("renders green double arrow when directed to no one and anyone acked (rule 1)", () => {
    const html = render({ ack: ["alice"], me: "eduardo", toTargets: [] });
    assert.ok(html.includes("text-green-500"), `expected green tone, got: ${html}`);
    assert.ok(html.includes("lucide-check-check"), `expected double arrow, got: ${html}`);
  });

  it("renders green double arrow when to=[X] and X acked (rule 2)", () => {
    const html = render({ ack: ["eduardo"], me: "eduardo", toTargets: ["eduardo"] });
    assert.ok(html.includes("text-green-500"), `expected green tone, got: ${html}`);
    assert.ok(html.includes("lucide-check-check"), `expected double arrow, got: ${html}`);
  });

  it("renders green single arrow when to=[me] and only an outsider acked (rule 3)", () => {
    const html = render({ ack: ["adrian"], me: "eduardo", toTargets: ["eduardo"] });
    assert.ok(html.includes("text-green-500"), `expected green tone, got: ${html}`);
    assert.ok(!html.includes("lucide-check-check"), `expected single arrow, got: ${html}`);
    assert.ok(html.includes("lucide-check"), `expected single check icon, got: ${html}`);
  });

  it("renders normal muted single arrow when nobody acked (rule 4)", () => {
    const html = render({ ack: [], me: "eduardo", toTargets: ["eduardo"] });
    assert.ok(!html.includes("text-green-500"), `expected muted tone, got: ${html}`);
    assert.ok(!html.includes("lucide-check-check"), `expected single arrow, got: ${html}`);
  });
});

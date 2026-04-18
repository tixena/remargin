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
import { AckButton } from "./AckButton.tsx";

// Minimal plugin + backend stand-ins, mirroring AckToggle.test.ts.
const pluginStub = { settings: DEFAULT_SETTINGS } as unknown as RemarginPlugin;
const backendStub = {
  registryShow: (): Promise<Participant[]> => Promise.resolve([]),
} as unknown as RemarginBackend;

const noop = (): void => {
  /* intentional — tests only inspect static markup */
};

function render(props: {
  ack: string[];
  me?: string | null;
  onAck?: () => void;
  toTargets?: string[];
}): string {
  __resetParticipantsCacheForTests();
  return renderToStaticMarkup(
    createElement(
      PluginContext.Provider,
      { value: pluginStub },
      createElement(
        BackendContext.Provider,
        { value: backendStub },
        createElement(AckButton, { onAck: noop, ...props })
      )
    )
  );
}

describe("AckButton", () => {
  it("renders a real <button> (not a span)", () => {
    const html = render({ ack: [], me: "eduardo" });
    assert.ok(html.startsWith("<button"), `expected <button>, got: ${html.slice(0, 80)}`);
    assert.ok(html.includes('type="button"'), "expected explicit type=button");
  });

  it("shows 'unacked' label when nobody has acked", () => {
    const html = render({ ack: [], me: "eduardo" });
    assert.ok(html.includes("unacked"), `expected 'unacked' label, got: ${html}`);
  });

  it("shows 'acked' label with count when only others acked", () => {
    const html = render({ ack: ["alice", "bob"], me: "eduardo" });
    assert.ok(html.includes(">acked<"), `expected 'acked' label, got: ${html}`);
    assert.ok(html.includes(">2<"), `expected count badge of 2, got: ${html}`);
  });

  it("surfaces the roster in the tooltip when there are acks", () => {
    const html = render({ ack: ["alice"], me: "eduardo" });
    assert.ok(html.includes('title="acked by alice"'), `expected roster tooltip, got: ${html}`);
  });

  it("falls back to a neutral tooltip when the ack list is empty", () => {
    const html = render({ ack: [], me: "eduardo" });
    assert.ok(html.includes('title="No acknowledgments yet"'), `got: ${html}`);
  });

  // Ack-visual precedence parity with AckToggle: the button must share
  // the same arrow/color rules so clicking it doesn't flip the visual.
  it("renders green double arrow when directed to no one and an outsider acked (rule 1)", () => {
    const html = render({ ack: ["alice"], me: "eduardo", toTargets: [] });
    assert.ok(html.includes("text-green-500"), `expected green tone, got: ${html}`);
    assert.ok(html.includes("lucide-check-check"), `expected double arrow, got: ${html}`);
  });

  it("renders green single arrow when directed to me and only an outsider acked (rule 3)", () => {
    const html = render({ ack: ["adrian"], me: "eduardo", toTargets: ["eduardo"] });
    assert.ok(html.includes("text-green-500"), `expected green tone, got: ${html}`);
    assert.ok(!html.includes("lucide-check-check"), `expected single arrow, got: ${html}`);
  });

  it("renders normal muted single arrow when nobody acked (rule 4)", () => {
    const html = render({ ack: [], me: "eduardo", toTargets: ["eduardo"] });
    assert.ok(!html.includes("text-green-500"), `expected muted tone, got: ${html}`);
    assert.ok(!html.includes("lucide-check-check"), `expected single arrow, got: ${html}`);
  });
});

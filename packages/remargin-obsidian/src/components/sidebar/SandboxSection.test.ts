import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { Participant, RemarginBackend, SandboxListEntry } from "../../backend/index.ts";
import type { ResolvedSystemPrompt } from "../../backend/types.ts";
import { BackendContext } from "../../hooks/useBackend.ts";
import { __resetParticipantsCacheForTests } from "../../hooks/useParticipants.ts";
import { PluginContext } from "../../hooks/usePlugin.ts";
import type RemarginPlugin from "../../main.ts";
import { DEFAULT_SETTINGS } from "../../types.ts";
import { SandboxSection } from "./SandboxSection.tsx";

// SSR skips useEffect, so only the pre-fetch loading state is reachable here.
// Post-fetch grouping is exercised by buildPromptGroups + PromptGroupSection.

const pluginStub = { settings: DEFAULT_SETTINGS } as unknown as RemarginPlugin;

interface BackendStubOptions {
  sandbox?: SandboxListEntry[];
  resolve?: (file: string) => ResolvedSystemPrompt;
  rejectSandboxList?: Error;
}

function makeBackend(opts: BackendStubOptions = {}): RemarginBackend {
  const { sandbox = [], resolve, rejectSandboxList } = opts;
  return {
    registryShow: (): Promise<Participant[]> => Promise.resolve([]),
    sandboxList: (): Promise<SandboxListEntry[]> =>
      rejectSandboxList ? Promise.reject(rejectSandboxList) : Promise.resolve(sandbox),
    sandboxRemove: (_files: string[]): Promise<void> => Promise.resolve(),
    resolvePrompt: (file: string): Promise<ResolvedSystemPrompt> =>
      Promise.resolve(
        resolve?.(file) ?? {
          is_default: true,
          name: "default",
          prompt: "",
          source: null,
        }
      ),
  } as unknown as RemarginBackend;
}

function render(backend: RemarginBackend, refreshKey = 0): string {
  __resetParticipantsCacheForTests();
  return renderToStaticMarkup(
    createElement(
      PluginContext.Provider,
      { value: pluginStub },
      createElement(
        BackendContext.Provider,
        { value: backend },
        createElement(SandboxSection, {
          refreshKey,
          viewMode: "flat",
          onOpenFile: () => undefined,
        })
      )
    )
  );
}

describe("SandboxSection — initial render", () => {
  it("renders the loading state before useEffect runs", () => {
    // SSR doesn't fire useEffect, so the initial fetch is never
    // dispatched. The component's `loading && files.length === 0`
    // branch renders the placeholder copy.
    const html = render(makeBackend());
    assert.ok(html.includes("Loading sandbox..."), `expected loading placeholder, got: ${html}`);
    assert.ok(!html.includes("Submit all"), "submit button must not appear in loading state");
  });

  it("does not throw when the backend rejects sandboxList (SSR safe)", () => {
    // The async fetch is never awaited under SSR — the rejection
    // bubble-up only matters at client mount time. The static markup
    // must still render the initial loading state without throwing.
    const html = render(makeBackend({ rejectSandboxList: new Error("CLI not installed") }));
    assert.ok(html.includes("Loading sandbox..."), `expected initial loading state, got: ${html}`);
  });

  it("renders a stable container that downstream tests can target", () => {
    const html = render(makeBackend());
    // The placeholder is the entire static markup — make sure it lives
    // inside a single root <div> (no fragment leakage).
    assert.ok(/^<div[^>]*>/.test(html), `expected single root div, got: ${html.slice(0, 100)}`);
  });
});

describe("SandboxSection — component contract", () => {
  it("the exported component is a function (renderable by React)", () => {
    // Simple shape lock — guards against accidental export regression
    // (e.g. an unintended default export).
    assert.equal(typeof SandboxSection, "function");
  });

  it("accepts and ignores an unknown refreshKey on the placeholder render", () => {
    // Smoke check: bumping the refresh key shouldn't change the
    // initial render (the effect hasn't run yet).
    const html0 = render(makeBackend(), 0);
    const html1 = render(makeBackend(), 99);
    assert.equal(html0, html1);
  });
});

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { Participant, RemarginBackend } from "../../backend/index.ts";
import type { Comment } from "../../generated/types.ts";
import { BackendContext } from "../../hooks/useBackend.ts";
import { __resetParticipantsCacheForTests } from "../../hooks/useParticipants.ts";
import { PluginContext } from "../../hooks/usePlugin.ts";
import type RemarginPlugin from "../../main.ts";
import { DEFAULT_SETTINGS } from "../../types.ts";
import { CommentHeader } from "./CommentHeader.tsx";

// Minimal plugin + backend stand-ins. `useParticipants` only reads
// `plugin.settings` (for the cache key) and calls `backend.registryShow()`,
// so we stub exactly those.
const pluginStub = { settings: DEFAULT_SETTINGS } as unknown as RemarginPlugin;
const backendStub = {
  registryShow: (): Promise<Participant[]> => Promise.resolve([]),
} as unknown as RemarginBackend;

function fixture(overrides: Partial<Comment>): Comment {
  return {
    ack: [],
    attachments: [],
    author: "alice",
    author_type: "human",
    checksum: "",
    content: "",
    edited_at: undefined,
    el: undefined,
    id: "oi5",
    line: 0,
    reactions: {},
    remargin_kind: [],
    reply_to: undefined,
    signature: undefined,
    sl: undefined,
    thread: undefined,
    to: [],
    ts: new Date("2026-04-14T12:00:00-04:00"),
    ...overrides,
  };
}

function render(comment: Comment): string {
  __resetParticipantsCacheForTests();
  return renderToStaticMarkup(
    createElement(
      PluginContext.Provider,
      { value: pluginStub },
      createElement(
        BackendContext.Provider,
        { value: backendStub },
        createElement(CommentHeader, { comment })
      )
    )
  );
}

describe("CommentHeader", () => {
  it("renders a badge containing the exact comment id", () => {
    const html = render(fixture({ id: "oi5" }));
    // Match the id-badge styling (bg-slate-500 text-white) with oi5 inside.
    assert.match(html, /<div[^>]*class="[^"]*bg-slate-500[^"]*text-white[^"]*"[^>]*>oi5<\/div>/);
  });

  it("renders the id verbatim for a different comment", () => {
    const html = render(fixture({ id: "xyz" }));
    assert.match(html, /<div[^>]*class="[^"]*bg-slate-500[^"]*text-white[^"]*"[^>]*>xyz<\/div>/);
    // Ensure the fixture id from the previous test didn't leak.
    assert.ok(!html.includes(">oi5<"), "expected previous id to be absent");
  });

  it("omits the id badge when comment.id is empty (defensive)", () => {
    const html = render(fixture({ id: "" }));
    assert.ok(!/bg-slate-500/.test(html), "expected no id badge when comment.id is empty");
  });

  it("renders one kind chip per remargin_kind, in stored order", () => {
    const html = render(fixture({ remargin_kind: ["decision-done", "action item"] }));
    const first = html.indexOf('aria-label="Kind: decision-done"');
    const second = html.indexOf('aria-label="Kind: action item"');
    assert.ok(first > -1 && second > first, html);
    assert.ok(html.includes(">decision-done</div>"), html);
  });

  it("renders no kind chip when the comment has no kind", () => {
    for (const remargin_kind of [[], undefined]) {
      const html = render(fixture({ remargin_kind }));
      assert.ok(!html.includes('aria-label="Kind:'), html);
    }
  });

  it("styles the kind chip apart from the id badge", () => {
    const html = render(fixture({ remargin_kind: ["decision-item"] }));
    assert.match(
      html,
      /<div[^>]*class="[^"]*rounded-full[^"]*"[^>]*aria-label="Kind: decision-item"/
    );
    assert.doesNotMatch(html, /<div[^>]*class="[^"]*bg-slate-500[^"]*"[^>]*aria-label="Kind:/);
  });
});

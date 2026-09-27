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
import { CommentCard } from "./CommentCard.tsx";

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
    content: "body",
    edited_at: undefined,
    el: undefined,
    id: "xuo",
    line: 104,
    reactions: {},
    remargin_kind: [],
    reply_to: undefined,
    signature: undefined,
    sl: undefined,
    thread: undefined,
    to: [],
    ts: new Date("2026-09-25T12:00:00Z"),
    ...overrides,
  };
}

const noop = () => {
  /* test-only no-op handler */
};

function render(comment: Comment): string {
  __resetParticipantsCacheForTests();
  return renderToStaticMarkup(
    createElement(
      PluginContext.Provider,
      { value: pluginStub },
      createElement(
        BackendContext.Provider,
        { value: backendStub },
        createElement(CommentCard, {
          comment,
          file: "notes/board.md",
          depth: 0,
          me: "bob",
          onAck: noop,
          onDelete: noop,
          onReact: noop,
        })
      )
    )
  );
}

describe("CommentCard", () => {
  it("puts the edited label and kind chips right after the add-reaction button", () => {
    const html = render(fixture({ edited_at: new Date(), remargin_kind: ["decision-done"] }));
    const picker = html.indexOf('aria-label="Add reaction"');
    const edited = html.indexOf('aria-label="Edited');
    const kind = html.indexOf('aria-label="Kind: decision-done"');
    const reply = html.indexOf(">Reply<");
    assert.ok(picker > -1 && edited > picker && kind > edited && reply > kind, html);
  });

  it("shows neither label on an unedited comment without kinds", () => {
    const html = render(fixture({}));
    assert.ok(!html.includes('aria-label="Edited'), html);
    assert.ok(!html.includes('aria-label="Kind:'), html);
  });
});

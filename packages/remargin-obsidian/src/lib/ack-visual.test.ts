/** Tests for the ack badge's arrow and tone rules. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { ackVisualFor } from "./ack-visual.ts";

describe("ackVisualFor", () => {
  it("is normal single arrow when nobody has acked", () => {
    assert.deepStrictEqual(ackVisualFor([], []), { arrow: "single", tone: "normal" });
    assert.deepStrictEqual(ackVisualFor(["alice"], []), {
      arrow: "single",
      tone: "normal",
    });
  });

  it("is green double arrow when directed to no one and acked by anyone", () => {
    assert.deepStrictEqual(ackVisualFor([], ["alice"]), {
      arrow: "double",
      tone: "green",
    });
    assert.deepStrictEqual(ackVisualFor([], ["alice", "bob"]), {
      arrow: "double",
      tone: "green",
    });
  });

  it("is green double arrow when directed to X and acked by X", () => {
    assert.deepStrictEqual(ackVisualFor(["eduardo"], ["eduardo"]), {
      arrow: "double",
      tone: "green",
    });
  });

  it("is green double arrow when any of the to-set has acked", () => {
    assert.deepStrictEqual(ackVisualFor(["eduardo", "bob"], ["alice", "eduardo"]), {
      arrow: "double",
      tone: "green",
    });
  });

  it("is green single arrow when directed to X and acked only by someone else", () => {
    assert.deepStrictEqual(ackVisualFor(["eduardo"], ["adrian"]), {
      arrow: "single",
      tone: "green",
    });
    assert.deepStrictEqual(ackVisualFor(["eduardo", "bob"], ["alice"]), {
      arrow: "single",
      tone: "green",
    });
  });

  it("prefers double-green when the to-set overlaps and an outsider also acked", () => {
    assert.deepStrictEqual(ackVisualFor(["eduardo"], ["eduardo", "adrian"]), {
      arrow: "double",
      tone: "green",
    });
  });

  it("falls back to normal when neither to nor ack has anything", () => {
    assert.deepStrictEqual(ackVisualFor([], []), {
      arrow: "single",
      tone: "normal",
    });
  });
});

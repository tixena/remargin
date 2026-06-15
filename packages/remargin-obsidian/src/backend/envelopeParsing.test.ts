import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import {
  Comment$Schema,
  ListEntry$Schema,
  ParticipantView$Schema,
  QueryResult$Schema,
  SandboxFailureEntry$Schema,
  SandboxListEntry$Schema,
  SearchMatch$Schema,
} from "@/generated";
import { parseVerifyFailure } from "@/lib/verifyFailure";
import { parsePayloadArray } from "./envelopeParsing";

// Every fixture below is REAL `remargin <cmd> --json` stdout captured from the
// CLI (not hand-authored), so each test proves the generated/checked schema
// accepts exactly what the binary emits — envelope metadata included. This is
// the regression guard for the `el`/`sl` (element graft) and `base_path`
// (envelope metadata) classes of "output did not match schema" failures.

const COMMENTS = `{
  "comments": [
    { "ack": [], "attachments": [], "author": "tester", "author_type": "human",
      "checksum": "sha256:abc123", "content": "A test comment mentioning notification.",
      "el": 18, "id": "abc", "line": 9, "reactions": {}, "sl": 9, "to": [],
      "ts": "2026-04-06T14:32:00-04:00" } ],
  "elapsed_ms": 2
}`;

const QUERY = `{
  "base_path": "doc.md/",
  "elapsed_ms": 1,
  "results": [
    { "comment_count": 1,
      "comments": [
        { "ack": [], "attachments": [], "author": "tester", "author_type": "human",
          "checksum": "sha256:abc123", "content": "A test comment mentioning notification.",
          "file": "doc.md", "id": "abc", "line": 16, "reactions": {}, "to": [],
          "ts": "2026-04-06T14:32:00-04:00" } ],
      "last_activity": "2026-04-06T14:32:00-04:00", "path": "doc.md", "pending_count": 1 } ]
}`;

const LS = `{
  "elapsed_ms": 1,
  "entries": [
    { "is_dir": false, "path": "doc.md", "remargin_last_activity": "2026-04-06T14:32:00-04:00",
      "remargin_pending": 1, "size": 407 },
    { "is_dir": true, "path": "sub" } ]
}`;

const SEARCH = `{
  "elapsed_ms": 4,
  "matches": [
    { "after": [], "before": [], "line": 7, "location": "Body", "path": "doc.md",
      "text": "Some body text with notification in it." },
    { "after": [], "before": [], "comment_id": "abc", "line": 17, "location": "Comment",
      "path": "doc.md", "text": "A test comment mentioning notification." } ]
}`;

const SANDBOX_LIST = `{
  "elapsed_ms": 2,
  "files": [ { "path": "doc.md", "since": "2026-06-15T13:55:38.579730993+00:00" } ]
}`;

const SANDBOX_REMOVE = `{
  "elapsed_ms": 2, "failed": [], "removed": ["doc.md"], "skipped": []
}`;

// Modeled on the documented registry shape (`registry_participant_json` in the
// CLI) — a registry must exist to capture it live, but the loose schema is what
// the plugin uses and this is the exact key set it produces.
const REGISTRY = `{
  "elapsed_ms": 1,
  "participants": [
    { "name": "alice", "display_name": "Alice", "type": "human", "status": "active", "pubkeys": 1 } ]
}`;

// Verify-gate refusal as it arrives on stderr (elapsed_ms injected on errors).
const VERIFY_REFUSAL = `{
  "elapsed_ms": 2,
  "error_kind": "verify_failed",
  "failures": [
    { "checksum_ok": false, "id": "abc", "recipients": "ok", "signature": "missing" } ],
  "headline": "verify failed (mode: strict)",
  "hint": "fix or sign the listed comments",
  "mode": "strict",
  "path": "doc.md"
}`;

describe("envelopeParsing — real CLI output parses", () => {
  it("comments: accepts the el/sl typed fields", () => {
    const comments = parsePayloadArray(COMMENTS, "comments", Comment$Schema, "comments");
    assert.equal(comments.length, 1);
    assert.equal(comments[0].id, "abc");
    assert.equal(comments[0].sl, 9);
    assert.equal(comments[0].el, 18);
  });

  it("query: tolerates the top-level base_path metadata", () => {
    const results = parsePayloadArray(QUERY, "results", QueryResult$Schema, "query");
    assert.equal(results.length, 1);
    assert.equal(results[0].path, "doc.md");
  });

  it("ls: parses entries with optional file metadata", () => {
    const entries = parsePayloadArray(LS, "entries", ListEntry$Schema, "ls");
    assert.equal(entries.length, 2);
    assert.equal(entries[1].is_dir, true);
  });

  it("search: parses matches with PascalCase location", () => {
    const matches = parsePayloadArray(SEARCH, "matches", SearchMatch$Schema, "search");
    assert.equal(matches.length, 2);
    assert.equal(matches[0].location, "Body");
    assert.equal(matches[1].comment_id, "abc");
  });

  it("sandbox list: parses files", () => {
    const files = parsePayloadArray(SANDBOX_LIST, "files", SandboxListEntry$Schema, "sandbox list");
    assert.equal(files.length, 1);
    assert.equal(files[0].path, "doc.md");
  });

  it("sandbox remove: validates the failure rows", () => {
    const failed = parsePayloadArray(
      SANDBOX_REMOVE,
      "failed",
      SandboxFailureEntry$Schema,
      "sandbox remove"
    );
    assert.equal(failed.length, 0);
  });

  it("registry show: parses participants", () => {
    const participants = parsePayloadArray(
      REGISTRY,
      "participants",
      ParticipantView$Schema,
      "registry show"
    );
    assert.equal(participants.length, 1);
    assert.equal(participants[0].name, "alice");
  });

  it("verify refusal: parses the verify_failed payload (elapsed_ms stripped)", () => {
    const parsed = parseVerifyFailure(VERIFY_REFUSAL);
    assert.ok(parsed);
    assert.equal(parsed.error_kind, "verify_failed");
    assert.equal(parsed.failures.length, 1);
    assert.equal(parsed.failures[0].id, "abc");
  });
});

describe("envelopeParsing — element strictness still bites", () => {
  // An un-modeled key inside an element must fail — this is exactly the guard
  // that the original `el`/`sl` graft tripped, and the reason base_path had to
  // be tolerated at the envelope level rather than by loosening elements.
  it("rejects an unknown key inside a comment element", () => {
    const bad = `{ "comments": [
      { "ack": [], "attachments": [], "author": "t", "author_type": "human",
        "checksum": "c", "content": "x", "id": "z", "line": 1, "reactions": {},
        "to": [], "ts": "2026-04-06T14:32:00-04:00", "bogus_field": 1 } ] }`;
    assert.throws(
      () => parsePayloadArray(bad, "comments", Comment$Schema, "comments"),
      /did not match schema/
    );
  });

  it("rejects an unknown key inside a search match element", () => {
    const bad = `{ "matches": [
      { "after": [], "before": [], "line": 1, "location": "Body", "path": "p",
        "text": "t", "bogus_field": 1 } ] }`;
    assert.throws(
      () => parsePayloadArray(bad, "matches", SearchMatch$Schema, "search"),
      /did not match schema/
    );
  });
});

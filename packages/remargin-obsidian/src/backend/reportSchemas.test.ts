/** Runs captured CLI report payloads through the generated schemas. */

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import {
  AuthorType$Schema,
  CpKind$Schema,
  CpOutcome$Schema,
  DoctorFinding$Schema,
  DoctorReport$Schema,
  ReplaceFileOutcome$Schema,
  ReplaceReport$Schema,
  SandboxListEntry$Schema,
  SandboxRemoveReport$Schema,
  Severity$Schema,
  VerifyErrorKind$Schema,
} from "@/generated";

// Every payload is real `remargin <cmd> --json` stdout. The io layer adds `elapsed_ms` to each
// top-level response, so whole-payload schemas admit that one key; nested fragments never see it.

const DOCTOR = `{
  "elapsed_ms": 24,
  "findings": [],
  "hook_installed": true,
  "project_settings_file": "/home/eduardoburgos/src/tixena/remargin/.claude/settings.json",
  "session_guard_installed": true,
  "user_settings_file": "/home/eduardoburgos/.claude/settings.json"
}`;

const REPLACE = `{
  "dry_run": false,
  "elapsed_ms": 2,
  "files": [ { "changed": true, "path": "doc.md", "replacements": 1 } ],
  "files_changed": 1,
  "files_failed": 0,
  "total_replacements": 1
}`;

const SANDBOX_REMOVE = `{
  "elapsed_ms": 2, "failed": [], "removed": ["doc.md"], "skipped": []
}`;

const SANDBOX_LIST_ENTRY = `{ "path": "doc.md", "since": "2026-08-02T14:10:26.961832093+00:00" }`;

describe("report schemas — live --json payloads parse whole", () => {
  it("doctor: parses the report the CLI actually emits", () => {
    const report = DoctorReport$Schema.parse(JSON.parse(DOCTOR));
    assert.equal(report.hook_installed, true);
    assert.equal(report.findings.length, 0);
    assert.equal(report.elapsed_ms, 24);
  });

  it("replace: parses the report with its nested file outcomes", () => {
    const report = ReplaceReport$Schema.parse(JSON.parse(REPLACE));
    assert.equal(report.total_replacements, 1);
    assert.equal(report.files[0].path, "doc.md");
    assert.equal(report.elapsed_ms, 2);
  });

  it("sandbox remove: parses the report", () => {
    const report = SandboxRemoveReport$Schema.parse(JSON.parse(SANDBOX_REMOVE));
    assert.deepStrictEqual(report.removed, ["doc.md"]);
    assert.equal(report.elapsed_ms, 2);
  });

  it("elapsed_ms is optional, so a payload without it still parses", () => {
    const payload = JSON.parse(DOCTOR);
    delete payload.elapsed_ms;
    const report = DoctorReport$Schema.parse(payload);
    assert.equal(report.elapsed_ms, undefined);
  });
});

describe("report schemas — strictness retained", () => {
  it("doctor: rejects an unknown key that is not elapsed_ms", () => {
    const bad = { ...JSON.parse(DOCTOR), bogus_field: 1 };
    assert.equal(DoctorReport$Schema.safeParse(bad).success, false);
  });

  it("replace: rejects an unknown key that is not elapsed_ms", () => {
    const bad = { ...JSON.parse(REPLACE), bogus_field: 1 };
    assert.equal(ReplaceReport$Schema.safeParse(bad).success, false);
  });

  it("sandbox remove: rejects an unknown key that is not elapsed_ms", () => {
    const bad = { ...JSON.parse(SANDBOX_REMOVE), bogus_field: 1 };
    assert.equal(SandboxRemoveReport$Schema.safeParse(bad).success, false);
  });

  it("doctor: rejects a non-numeric elapsed_ms", () => {
    const bad = { ...JSON.parse(DOCTOR), elapsed_ms: "24" };
    assert.equal(DoctorReport$Schema.safeParse(bad).success, false);
  });
});

describe("nested schemas — no elapsed_ms member", () => {
  it("replace file outcome parses on its own but rejects elapsed_ms", () => {
    const element = JSON.parse(REPLACE).files[0];
    assert.equal(ReplaceFileOutcome$Schema.parse(element).replacements, 1);
    assert.equal(ReplaceFileOutcome$Schema.safeParse({ ...element, elapsed_ms: 2 }).success, false);
  });

  it("sandbox list entry parses on its own but rejects elapsed_ms", () => {
    const element = JSON.parse(SANDBOX_LIST_ENTRY);
    assert.equal(SandboxListEntry$Schema.parse(element).path, "doc.md");
    assert.equal(SandboxListEntry$Schema.safeParse({ ...element, elapsed_ms: 2 }).success, false);
  });
});

// serde emits enum variants in `rename_all` casing; the schemas must carry those exact strings.

const CP_VERBATIM = `{
  "bytes_copied": 69,
  "comments_dropped": 0,
  "dst_absolute": "/tmp/claude-1000/-home-eduardoburgos-src-tixena-remargin/67951bdc-457f-48df-9a53-621de3f2f183/scratchpad/jwa6/realm/verbatim_copy.md",
  "elapsed_ms": 2,
  "kind": "verbatim",
  "overwritten": false,
  "src_absolute": "/tmp/claude-1000/-home-eduardoburgos-src-tixena-remargin/67951bdc-457f-48df-9a53-621de3f2f183/scratchpad/jwa6/realm/doc.md"
}`;

const CP_BODY_ONLY = `{
  "bytes_copied": 506,
  "comments_dropped": 1,
  "dst_absolute": "/tmp/claude-1000/-home-eduardoburgos-src-tixena-remargin/67951bdc-457f-48df-9a53-621de3f2f183/scratchpad/jwa6/realm/body_copy.md",
  "elapsed_ms": 2,
  "kind": "body_only",
  "overwritten": true,
  "src_absolute": "/tmp/claude-1000/-home-eduardoburgos-src-tixena-remargin/67951bdc-457f-48df-9a53-621de3f2f183/scratchpad/jwa6/realm/doc.md"
}`;

const DOCTOR_WITH_FINDING = `{
  "elapsed_ms": 1,
  "findings": [
    {
      "kind": "hook_missing",
      "message": "The PreToolUse hook (\`remargin claude pretool\`) is not registered in either the user-scope settings (/tmp/claude-1000/-home-eduardoburgos-src-tixena-remargin/67951bdc-457f-48df-9a53-621de3f2f183/scratchpad/jwa6/home/.claude/settings.json) or the project-scope settings (/tmp/claude-1000/-home-eduardoburgos-src-tixena-remargin/67951bdc-457f-48df-9a53-621de3f2f183/scratchpad/jwa6/realm/.claude/settings.json). No enforcement is active — agents can invoke the remargin CLI and bypass path restrictions without restriction.",
      "remedy": "Run \`remargin claude pretool install\` to register the hook.",
      "severity": "critical"
    }
  ],
  "hook_installed": false,
  "project_settings_file": "/tmp/claude-1000/-home-eduardoburgos-src-tixena-remargin/67951bdc-457f-48df-9a53-621de3f2f183/scratchpad/jwa6/realm/.claude/settings.json",
  "session_guard_installed": false,
  "user_settings_file": "/tmp/claude-1000/-home-eduardoburgos-src-tixena-remargin/67951bdc-457f-48df-9a53-621de3f2f183/scratchpad/jwa6/home/.claude/settings.json"
}`;

describe("enum wire casing — live payloads carry serde's snake_case", () => {
  it("cp verbatim: the whole payload parses and kind is snake_case", () => {
    const outcome = CpOutcome$Schema.parse(JSON.parse(CP_VERBATIM));
    assert.equal(outcome.kind, "verbatim");
    assert.equal(outcome.comments_dropped, 0);
  });

  it("cp body_only: the whole payload parses and kind is snake_case", () => {
    const outcome = CpOutcome$Schema.parse(JSON.parse(CP_BODY_ONLY));
    assert.equal(outcome.kind, "body_only");
    assert.equal(outcome.comments_dropped, 1);
  });

  it("doctor: a non-empty findings array parses with snake_case kind and severity", () => {
    const report = DoctorReport$Schema.parse(JSON.parse(DOCTOR_WITH_FINDING));
    assert.equal(report.findings.length, 1);
    assert.equal(report.findings[0].kind, "hook_missing");
    assert.equal(report.findings[0].severity, "critical");
  });

  it("the raw Rust identifiers the old generator emitted are rejected", () => {
    assert.equal(CpKind$Schema.safeParse("Verbatim").success, false);
    assert.equal(CpKind$Schema.safeParse("BodyOnly").success, false);
    assert.equal(Severity$Schema.safeParse("Critical").success, false);
    const finding = JSON.parse(DOCTOR_WITH_FINDING).findings[0];
    assert.equal(
      DoctorFinding$Schema.safeParse({ ...finding, kind: "HookMissing" }).success,
      false
    );
  });

  it("enums that were already correct are unchanged", () => {
    assert.equal(AuthorType$Schema.parse("agent"), "agent");
    assert.equal(AuthorType$Schema.parse("human"), "human");
    assert.equal(VerifyErrorKind$Schema.parse("verify_failed"), "verify_failed");
  });
});

//! CLI E2E tests for the recipient registry gate.

use core::str;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const REGISTRY_YAML: &str = "\
participants:
  alice:
    type: human
    status: active
    pubkeys: []
  bob:
    type: human
    status: revoked
    pubkeys: []
  agent-x:
    type: agent
    status: active
    pubkeys: []
";

const BODY: &str = "---\ntitle: Strict realm\n---\n\n# Title\n\nBody.\n";

const DOC_UNKNOWN_RECIPIENT: &str = "\
---
title: Test
---

```remargin
---
id: abc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
to: [eduardo_burgos]
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```
";

const DOC_ACTIVE_RECIPIENT: &str = "\
---
title: Test
---

```remargin
---
id: abc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
to: [alice]
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```
";

const DOC_OPEN_MODE_RECIPIENT: &str = "\
---
title: Test
---

```remargin
---
id: abc
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
to: [anyone_at_all]
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```
";

/// Build a registered-mode realm with the recipient registry. Returns the doc path.
fn build_registered_realm(tmp: &TempDir) -> PathBuf {
    let realm = tmp.path().join("realm");
    fs::create_dir_all(&realm).unwrap();
    fs::write(
        realm.join(".remargin.yaml"),
        "identity: agent-x\ntype: agent\nmode: registered\n",
    )
    .unwrap();
    fs::write(realm.join(".remargin-registry.yaml"), REGISTRY_YAML).unwrap();
    let doc = realm.join("doc.md");
    fs::write(&doc, BODY).unwrap();
    doc
}

/// Parse the stdout JSON or fail the test with the raw output.
fn parse_json(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap()
}

/// Get the `recipients` array from a lint JSON value or fail the test.
fn recipients_array(value: &Value) -> &Vec<Value> {
    value.get("recipients").and_then(Value::as_array).unwrap()
}

fn run(cwd: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn cli_comment_to_unknown_recipient_rejected_in_registered_realm() {
    let tmp = TempDir::new().unwrap();
    let doc = build_registered_realm(&tmp);
    let doc_str = doc.to_string_lossy();

    let out = run(
        &tmp.path().join("realm"),
        &[
            "comment",
            &doc_str,
            "test comment",
            "--to",
            "eduardo_burgos",
        ],
    );

    assert!(
        !out.status.success(),
        "comment with unknown recipient must exit non-zero; \
         stdout={}\nstderr={}",
        str::from_utf8(&out.stdout).unwrap_or_default(),
        str::from_utf8(&out.stderr).unwrap_or_default(),
    );

    let stderr = str::from_utf8(&out.stderr).unwrap_or_default();
    assert!(
        stderr.contains("eduardo_burgos") || stderr.contains("recipient"),
        "error must name the bad recipient or say 'recipient': {stderr}"
    );

    let after = fs::read_to_string(&doc).unwrap();
    assert!(
        !after.contains("```remargin"),
        "doc must not have been mutated on refusal:\n{after}"
    );
}

#[test]
fn cli_comment_to_active_recipient_allowed_in_registered_realm() {
    let tmp = TempDir::new().unwrap();
    let doc = build_registered_realm(&tmp);
    let doc_str = doc.to_string_lossy();

    let out = run(
        &tmp.path().join("realm"),
        &[
            "comment",
            &doc_str,
            "test comment for alice",
            "--to",
            "alice",
        ],
    );

    assert!(
        out.status.success(),
        "comment to active recipient must succeed; \
         stdout={}\nstderr={}",
        str::from_utf8(&out.stdout).unwrap_or_default(),
        str::from_utf8(&out.stderr).unwrap_or_default(),
    );

    let after = fs::read_to_string(&doc).unwrap();
    assert!(
        after.contains("```remargin"),
        "doc should contain a comment block after success:\n{after}"
    );
}

/// The report carries `ok:false` and a non-empty `recipients` array.
#[test]
fn cli_lint_json_reports_unknown_recipient_finding() {
    let tmp = TempDir::new().unwrap();
    let realm = tmp.path().join("realm");
    fs::create_dir_all(&realm).unwrap();
    fs::write(realm.join(".remargin.yaml"), "mode: registered\n").unwrap();
    fs::write(realm.join(".remargin-registry.yaml"), REGISTRY_YAML).unwrap();
    let doc = realm.join("doc.md");
    fs::write(&doc, DOC_UNKNOWN_RECIPIENT).unwrap();
    let doc_str = doc.to_string_lossy();

    let out = run(&realm, &["lint", "--json", &doc_str]);

    assert!(
        !out.status.success(),
        "lint should exit non-zero for unknown recipient; \
         stdout={}\nstderr={}",
        str::from_utf8(&out.stdout).unwrap_or_default(),
        str::from_utf8(&out.stderr).unwrap_or_default(),
    );

    let stdout = str::from_utf8(&out.stdout).unwrap();
    let value = parse_json(stdout);

    assert_eq!(value.get("ok").and_then(Value::as_bool), Some(false));

    let recipients = recipients_array(&value);
    assert!(
        !recipients.is_empty(),
        "recipients array should be non-empty; json={value}"
    );

    let message = recipients[0]
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        message.contains("eduardo_burgos"),
        "finding should name the bad recipient: {message}"
    );
}

/// `lint --json` on a doc with only active recipients → `ok:true`, empty `recipients`.
#[test]
fn cli_lint_json_active_recipients_no_findings() {
    let tmp = TempDir::new().unwrap();
    let realm = tmp.path().join("realm");
    fs::create_dir_all(&realm).unwrap();
    fs::write(realm.join(".remargin.yaml"), "mode: registered\n").unwrap();
    fs::write(realm.join(".remargin-registry.yaml"), REGISTRY_YAML).unwrap();
    let doc = realm.join("doc.md");
    fs::write(&doc, DOC_ACTIVE_RECIPIENT).unwrap();
    let doc_str = doc.to_string_lossy();

    let out = run(&realm, &["lint", "--json", &doc_str]);

    let stdout = str::from_utf8(&out.stdout).unwrap();
    let value = parse_json(stdout);

    let recipients = recipients_array(&value);
    assert!(
        recipients.is_empty(),
        "no recipient findings expected for active recipient; json={value}"
    );
    assert_eq!(
        value.get("ok").and_then(Value::as_bool),
        Some(true),
        "ok should be true for active recipients"
    );
}

/// `lint --json` in open mode → no recipient findings regardless of `to:` content.
#[test]
fn cli_lint_json_open_mode_no_recipient_findings() {
    let tmp = TempDir::new().unwrap();
    let doc = tmp.path().join("doc.md");
    fs::write(&doc, DOC_OPEN_MODE_RECIPIENT).unwrap();
    let doc_str = doc.to_string_lossy();

    let out = run(tmp.path(), &["lint", "--json", &doc_str]);

    let stdout = str::from_utf8(&out.stdout).unwrap();
    let value = parse_json(stdout);

    let recipients = recipients_array(&value);
    assert!(
        recipients.is_empty(),
        "open mode: no recipient findings expected; json={value}"
    );
}

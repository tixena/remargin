use core::str;
use std::fs;
use std::path::Path;
use std::process::Output;

use assert_cmd::Command;
use os_shim::System as _;
use os_shim::real::RealSystem;
use remargin_core::config::ResolvedConfig;
use remargin_core::config::identity::IdentityFlags;
use remargin_core::config::parse_author_type;
use remargin_core::mcp;
use serde_json::{Value, json};
use tempfile::TempDir;

fn realm_with(files: &[(&str, &str)]) -> TempDir {
    let realm = TempDir::new().unwrap();
    fs::write(
        realm.path().join(".remargin.yaml"),
        "identity: alice\ntype: human\n",
    )
    .unwrap();
    for (rel, body) in files {
        let path = realm.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
    }
    realm
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

fn assert_status(out: &Output, expected: i32) {
    let actual = out.status.code();
    assert_eq!(
        actual,
        Some(expected),
        "remargin exited with {:?}\nstdout: {}\nstderr: {}",
        actual,
        str::from_utf8(&out.stdout).unwrap(),
        str::from_utf8(&out.stderr).unwrap(),
    );
}

fn doc(id: &str, author: &str, ts: &str) -> String {
    format!(
        "---\ntitle: t\n---\n\n# Body\n\n```remargin\n---\nid: {id}\nauthor: {author}\ntype: human\nts: {ts}\nchecksum: sha256:t\n---\nBody.\n```\n"
    )
}

/// JSON output is the default: `remargin activity` returns
/// the structured `ActivityResult` as pretty-printed JSON on
/// stdout.
#[test]
fn json_output_is_default() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &["activity", "--identity", "alice", "--type", "human"],
    );
    assert_status(&out, 0);
    let stdout = str::from_utf8(&out.stdout).unwrap();
    let value: Value = serde_json::from_str(stdout).unwrap();
    let files = value["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    let changes = files[0]["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["kind"], json!("comment"));
}

/// `--pretty` switches to the human-readable timeline; output
/// goes to stderr so stdout stays clean for CLI piping.
///: each per-file block opens with a cutoff header so
/// the reader can tell which timeline they are looking at; the
/// initial-touch fallback (caller has no prior activity in the
/// file) renders the explicit "since the beginning" wording.
#[test]
fn pretty_output_renders_timeline() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--pretty",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_status(&out, 0);
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(stderr.contains("comment"), "{stderr}");
    assert!(stderr.contains("c1 by bob"), "{stderr}");
    assert!(
        stderr.contains("since the beginning"),
        "expected initial-touch fallback header in: {stderr}"
    );
    assert!(
        !stderr.contains("YOUR-LAST-ACTION"),
        "header must not leak the placeholder string: {stderr}"
    );
}

///: explicit `--since` echoes the cutoff in the
/// `--pretty` header line so the reader can confirm it.
#[test]
fn pretty_output_renders_explicit_since_header() {
    // Use a future-enough cutoff so something is filtered, but
    // also keep a comment after the cutoff so the per-file
    // block (and its header) is rendered.
    let realm = realm_with(&[
        ("a.md", &doc("c1", "bob", "2026-04-08T12:00:00-04:00")),
        ("b.md", &doc("c2", "bob", "2026-04-06T12:00:00-04:00")),
    ]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--pretty",
            "--since",
            "2026-04-07T00:00:00-04:00",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_status(&out, 0);
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(
        stderr.contains("(since 2026-04-07 00:00)"),
        "expected explicit-since header in: {stderr}"
    );
}

/// `--since` parses ISO 8601 and applies as an explicit
/// cutoff. A comment before the cutoff is dropped.
#[test]
fn since_cutoff_filters_comments() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--since",
            "2026-04-06T13:00:00-04:00",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_status(&out, 0);
    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert!(value["files"].as_array().unwrap().is_empty());
}

/// `--since` with malformed input errors with a clear
/// message.
#[test]
fn malformed_since_errors() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--since",
            "not-a-date",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_ne!(out.status.code(), Some(0_i32));
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(stderr.contains("--since"), "{stderr}");
}

/// `--pretty` and `--json` together is rejected.
#[test]
fn pretty_and_json_are_mutually_exclusive() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--pretty",
            "--json",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_ne!(out.status.code(), Some(0_i32));
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(stderr.contains("mutually exclusive"), "{stderr}");
}

/// MCP parity: `mcp__remargin__activity` (compact, hardcoded) matches the
/// CLI `--json --compact` payload. The change rows are path-independent, so
/// they must be equal element-wise across both surfaces.
#[test]
fn mcp_activity_matches_cli_compact_shape() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let cli = run_in(
        realm.path(),
        &[
            "activity",
            "--json",
            "--compact",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_status(&cli, 0);
    let cli_payload: Value = serde_json::from_str(str::from_utf8(&cli.stdout).unwrap()).unwrap();

    let system = RealSystem::new();
    let base = system.canonicalize(realm.path()).unwrap();
    let mut flags = IdentityFlags::default();
    flags.identity = Some(String::from("alice"));
    flags.author_type = Some(parse_author_type("human").unwrap());
    let config = ResolvedConfig::resolve(&system, &base, &flags, None).unwrap();
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/call",
        "params": {
            "name": "activity",
            "arguments": {}
        }
    });
    let request_str = serde_json::to_string(&request).unwrap();
    let response_str = mcp::process_request(&system, &base, &config, &request_str)
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_str(&response_str).unwrap();
    let result = response.get("result").unwrap();
    let content = result.get("content").and_then(Value::as_array).unwrap();
    let text = content[0].get("text").and_then(Value::as_str).unwrap();
    let mcp_payload: Value = serde_json::from_str(text).unwrap();

    // Same columnar header + envelope flags on both surfaces.
    assert_eq!(cli_payload["change_cols"], mcp_payload["change_cols"]);
    assert_eq!(
        cli_payload["cutoff_explicit"],
        mcp_payload["cutoff_explicit"]
    );

    let cli_files = cli_payload["files"].as_array().unwrap();
    let mcp_files = mcp_payload["files"].as_array().unwrap();
    assert_eq!(cli_files.len(), 1);
    assert_eq!(mcp_files.len(), 1);
    // Change rows carry no path — they must match element-wise.
    assert_eq!(cli_files[0]["changes"], mcp_files[0]["changes"]);
    let row = cli_files[0]["changes"][0].as_array().unwrap();
    assert_eq!(row.len(), 9);
    assert_eq!(row[1], json!("comment"));
    assert_eq!(row[4], json!("c1"));
}

/// `--json --compact` emits the columnar payload minified: a single output
/// line, positional 9-column rows under a `change_cols` header.
#[test]
fn cli_activity_compact_minified_columnar() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--json",
            "--compact",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_status(&out, 0);
    let raw = str::from_utf8(&out.stdout).unwrap();
    // Minified: only the trailing newline breaks the single payload line.
    assert_eq!(
        raw.trim_end_matches('\n').lines().count(),
        1,
        "compact payload must be minified: {raw:?}"
    );

    let payload: Value = serde_json::from_str(raw.trim()).unwrap();
    let cols = payload["change_cols"].as_array().unwrap();
    assert_eq!(cols.len(), 9);
    assert_eq!(cols[0], "ts");
    assert_eq!(cols[1], "kind");

    let rows = payload["files"][0]["changes"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = rows[0].as_array().unwrap();
    assert_eq!(row.len(), 9, "positional row, no named keys: {row:?}");
    assert_eq!(row[1], json!("comment"));
    assert_eq!(row[4], json!("c1"));
}

/// Regression: `--json` (no `--compact`) keeps today's verbose, pretty
/// payload — tagged `Change` objects with named fields. Compact must not
/// leak in.
#[test]
fn cli_activity_verbose_json_unchanged() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--json",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_status(&out, 0);
    let raw = str::from_utf8(&out.stdout).unwrap();
    // Verbose stays pretty-printed (multi-line).
    assert!(raw.lines().count() > 3, "pretty-printed: {raw:?}");

    let payload: Value = serde_json::from_str(raw).unwrap();
    assert!(payload.get("change_cols").is_none(), "no columnar header");
    let change = &payload["files"][0]["changes"][0];
    assert!(change.is_object(), "verbose change is an object: {change}");
    assert_eq!(change["kind"], json!("comment"));
    assert_eq!(change["comment_id"], json!("c1"));
    assert!(change.get("line_start").is_some());
}

/// `--compact` without `--json` is a clap-level error (requires `--json`).
#[test]
fn compact_requires_json() {
    let realm = realm_with(&[("note.md", &doc("c1", "bob", "2026-04-06T12:00:00-04:00"))]);
    let out = run_in(
        realm.path(),
        &[
            "activity",
            "--compact",
            "--identity",
            "alice",
            "--type",
            "human",
        ],
    );
    assert_ne!(out.status.code(), Some(0_i32));
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(
        stderr.contains("--json"),
        "clap requires error must name --json: {stderr}"
    );
}

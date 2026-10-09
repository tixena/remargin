//! Runs the read subcommands against a strict realm as admitted and refused callers.

use core::str;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use tempfile::TempDir;

const TEST_PRIVATE_KEY: &str = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
";

const TEST_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILVfufIVR1+wwXvHwYjjSVOO1PyMrur+yoibLd5o/hmV test@remargin";

const OPEN_DOC: &str = "\
---
title: Open doc
---

# Open doc

Needle open text.
";

const STRICT_DOC: &str = "\
---
title: Read doc
---

# Read doc

Needle body text.
";

/// Run with the strict realm as the working directory.
const READ_SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("get", &["doc.md"]),
    ("ls", &["."]),
    ("comments", &["doc.md"]),
    ("query", &["."]),
    ("search", &["Needle"]),
    ("metadata", &["doc.md"]),
    ("lint", &["doc.md"]),
    ("verify", &["doc.md"]),
];

/// Run from the neighbouring open realm, naming the strict realm in the argument.
const CROSS_REALM_SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("get", &["strict/doc.md"]),
    ("ls", &["strict"]),
    ("comments", &["strict/doc.md"]),
    ("query", &["strict/doc.md"]),
    ("search", &["Needle", "--path", "strict/doc.md"]),
    ("metadata", &["strict/doc.md"]),
    ("lint", &["strict/doc.md"]),
    ("verify", &["strict/doc.md"]),
];

fn registry_yaml(status: &str) -> String {
    format!(
        "participants:\n  alice:\n    type: human\n    status: {status}\n    pubkeys:\n      - {TEST_PUBLIC_KEY}\n"
    )
}

/// ```text
/// tmp/
/// ├── .remargin.yaml            identity: mallory, type: human, mode: open
/// ├── .remargin-registry.yaml   alice, active
/// ├── alice.yaml                alice's identity + signing key
/// ├── alice_key
/// ├── open.md                   carries a comment so walks surface it
/// └── strict/
///     ├── .remargin.yaml        mode: strict, no identity
///     └── doc.md                carries a comment signed by alice
/// ```
///
/// Both comments are written through the CLI so the strict-realm one
/// carries a real signature and `verify` reports the realm clean.
fn build_layout() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    fs::write(
        root.join(".remargin-registry.yaml"),
        registry_yaml("active"),
    )
    .unwrap();
    fs::write(root.join("alice_key"), TEST_PRIVATE_KEY).unwrap();
    fs::write(
        root.join("alice.yaml"),
        "identity: alice\ntype: human\nkey: ./alice_key\n",
    )
    .unwrap();
    fs::write(
        root.join(".remargin.yaml"),
        "identity: mallory\ntype: human\nmode: open\n",
    )
    .unwrap();
    fs::write(root.join("open.md"), OPEN_DOC).unwrap();

    let strict = root.join("strict");
    fs::create_dir_all(&strict).unwrap();
    fs::write(strict.join(".remargin.yaml"), "mode: strict\n").unwrap();
    fs::write(strict.join("doc.md"), STRICT_DOC).unwrap();

    run_ok(&root, &["comment", "open.md", "Open realm note."]);
    let config = alice_config(&root);
    run_ok(
        &strict,
        &[
            "comment",
            "doc.md",
            "Strict realm note.",
            "--config",
            &config,
        ],
    );

    (tmp, root)
}

/// Absolute path to the `--config` file that resolves alice, whose key
/// and registry entry both live at the layout root.
fn alice_config(root: &Path) -> String {
    String::from(root.join("alice.yaml").to_string_lossy())
}

/// Rewrite the registry with alice's status flipped to `revoked`.
fn revoke_alice(root: &Path) {
    fs::write(
        root.join(".remargin-registry.yaml"),
        registry_yaml("revoked"),
    )
    .unwrap();
}

fn run(cwd: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

fn run_ok(cwd: &Path, args: &[&str]) -> String {
    let out = run(cwd, args);
    let stderr = str::from_utf8(&out.stderr).unwrap();
    let stdout = str::from_utf8(&out.stdout).unwrap();
    assert!(
        out.status.success(),
        "remargin {args:?} failed\nstderr: {stderr}\nstdout: {stdout}"
    );
    String::from(stdout)
}

/// Assert the invocation was refused, named `caller` in its diagnostic,
/// and leaked no document text.
fn assert_refused(cwd: &Path, args: &[&str], caller: &str) {
    let out = run(cwd, args);
    let stdout = str::from_utf8(&out.stdout).unwrap();
    let stderr = str::from_utf8(&out.stderr).unwrap();
    assert!(
        !out.status.success(),
        "remargin {args:?} must refuse {caller}\nstdout: {stdout}"
    );
    assert!(
        stderr.contains(caller),
        "stderr must name the caller {caller}, got: {stderr}"
    );
    assert!(
        !stdout.contains("Needle"),
        "no document text may leak, got: {stdout}"
    );
}

#[test]
fn anonymous_reads_in_strict_realm_are_refused() {
    let (_tmp, root) = build_layout();
    let strict_dir = root.join("strict");
    for &(cmd, tail) in READ_SUBCOMMANDS {
        let mut args = vec![cmd];
        args.extend_from_slice(tail);
        assert_refused(&strict_dir, &args, "<anonymous>");
    }
}

#[test]
fn unregistered_caller_from_open_realm_cannot_read_strict_doc() {
    let (_tmp, root) = build_layout();
    for &(cmd, tail) in CROSS_REALM_SUBCOMMANDS {
        let mut args = vec![cmd];
        args.extend_from_slice(tail);
        assert_refused(&root, &args, "mallory");
    }
}

#[test]
fn cp_out_of_strict_realm_is_refused_for_unregistered_caller() {
    let (_tmp, root) = build_layout();
    assert_refused(&root, &["cp", "strict/doc.md", "copy.md"], "mallory");
    assert!(
        !root.join("copy.md").exists(),
        "a refused cp must not create the destination"
    );
}

#[test]
fn mv_out_of_strict_realm_is_refused_for_unregistered_caller() {
    let (_tmp, root) = build_layout();
    assert_refused(&root, &["mv", "strict/doc.md", "moved.md"], "mallory");
    assert!(
        !root.join("moved.md").exists(),
        "a refused mv must not create the destination"
    );
    assert!(
        root.join("strict").join("doc.md").exists(),
        "a refused mv must leave the source in place"
    );
}

#[test]
fn revoked_participant_is_refused() {
    let (_tmp, root) = build_layout();
    revoke_alice(&root);
    let config = alice_config(&root);
    assert_refused(
        &root,
        &["get", "strict/doc.md", "--config", &config],
        "alice",
    );
}

#[test]
fn registered_caller_reads_strict_realm() {
    let (_tmp, root) = build_layout();
    let strict_dir = root.join("strict");
    let config = alice_config(&root);
    for &(cmd, tail) in READ_SUBCOMMANDS {
        let mut args = vec![cmd];
        args.extend_from_slice(tail);
        args.extend_from_slice(&["--config", &config]);
        run_ok(&strict_dir, &args);
    }
    let body = run_ok(&strict_dir, &["get", "doc.md", "--config", &config]);
    assert!(
        body.contains("Needle body text."),
        "an active participant must read the body unchanged, got: {body}"
    );
}

#[test]
fn walk_skips_nested_strict_realm() {
    let (_tmp, root) = build_layout();

    let searched = run_ok(&root, &["search", "Needle"]);
    assert!(
        searched.contains("open.md"),
        "the admitted open realm must still be searched, got: {searched}"
    );
    assert!(
        !searched.contains("doc.md"),
        "the nested strict realm must be omitted, got: {searched}"
    );

    let queried = run_ok(&root, &["query", "."]);
    assert!(
        queried.contains("open.md"),
        "the admitted open realm must still be queried, got: {queried}"
    );
    assert!(
        !queried.contains("doc.md"),
        "the nested strict realm must be omitted, got: {queried}"
    );
}

#[test]
fn open_realm_reads_are_unchanged_for_an_anonymous_caller() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    fs::write(root.join(".remargin.yaml"), "mode: open\n").unwrap();
    fs::write(root.join("open.md"), OPEN_DOC).unwrap();

    let body = run_ok(&root, &["get", "open.md"]);
    assert!(
        body.contains("Needle open text."),
        "open-mode reads stay anonymous-friendly, got: {body}"
    );
    run_ok(&root, &["ls", "."]);
    run_ok(&root, &["search", "Needle"]);
}

#[test]
fn identity_flags_reach_metadata_get_image_and_lint() {
    let (_tmp, root) = build_layout();
    let config = alice_config(&root);
    let strict_dir = root.join("strict");
    for &(cmd, tail) in &[
        ("metadata", &["doc.md"][..]),
        ("lint", &["doc.md"][..]),
        ("get-image", &["missing.png"][..]),
    ] {
        let mut args = vec![cmd];
        args.extend_from_slice(tail);
        args.extend_from_slice(&["--config", &config]);
        let out = run(&strict_dir, &args);
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            !stderr.contains("unexpected argument"),
            "remargin {args:?} must accept --config, got: {stderr}"
        );
    }
}

use core::str;
use std::fs;
use std::path::Path;
use std::process::Output;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

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

/// A managed directory with mixed visible contents is removed
/// recursively; the JSON report names the files and folders.
#[test]
fn removes_directory_tree_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("tree/sub")).unwrap();
    fs::write(realm.path().join("tree/top.md"), b"top").unwrap();
    fs::write(realm.path().join("tree/sub/nested.txt"), b"nested").unwrap();

    let out = run_in(realm.path(), &["rm", "tree", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["is_directory"], true);
    assert_eq!(value["files_deleted"].as_array().unwrap().len(), 2);
    assert_eq!(value["folders_removed"].as_array().unwrap().len(), 2);
    assert_eq!(value["folders_left_behind"].as_array().unwrap().len(), 0);

    assert!(!realm.path().join("tree").exists());
}

/// A directory holding a hidden file: the visible file is removed, the
/// folder is left behind (still holds the dotfile), and no error.
#[test]
fn leaves_folder_with_hidden_file_behind_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("box")).unwrap();
    fs::write(realm.path().join("box/visible.md"), b"v").unwrap();
    fs::write(realm.path().join("box/.secret"), b"hidden").unwrap();

    let out = run_in(realm.path(), &["rm", "box", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["files_deleted"].as_array().unwrap().len(), 1);
    assert_eq!(value["folders_left_behind"].as_array().unwrap().len(), 1);

    assert!(!realm.path().join("box/visible.md").exists());
    assert!(realm.path().join("box/.secret").exists());
    assert!(realm.path().join("box").exists());
}

/// All-or-nothing: an unreadable file in the tree aborts the call before
/// any deletion. The whole tree stays intact.
#[cfg(unix)]
#[test]
fn unreadable_file_aborts_and_leaves_tree_intact() {
    use std::os::unix::fs::PermissionsExt as _;

    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("docs")).unwrap();
    fs::write(realm.path().join("docs/ok.md"), b"ok").unwrap();
    let bad = realm.path().join("docs/bad.md");
    fs::write(&bad, b"bad").unwrap();
    // A `000`-mode file stats fine but cannot be opened for read; the
    // open-based readability pre-flight trips on it and aborts the call.
    let mut perms = fs::metadata(&bad).unwrap().permissions();
    perms.set_mode(0o000);
    fs::set_permissions(&bad, perms).unwrap();

    let out = run_in(realm.path(), &["rm", "docs", "--json"]);

    // Restore perms so TempDir cleanup can delete the file.
    let mut restore = fs::metadata(&bad).unwrap().permissions();
    restore.set_mode(0o644);
    fs::set_permissions(&bad, restore).unwrap();

    assert_ne!(out.status.code(), Some(0_i32), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bad.md"),
        "error must name the blocking path, got: {stderr}"
    );
    // Nothing deleted.
    assert!(realm.path().join("docs/ok.md").exists());
    assert!(realm.path().join("docs/bad.md").exists());
}

/// A symlink inside the tree pointing outside is unlinked, not followed:
/// the link's target survives.
#[cfg(unix)]
#[test]
fn symlink_is_unlinked_not_followed_via_cli() {
    use std::os::unix::fs::symlink;

    let realm = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let target = outside.path().join("target.md");
    fs::write(&target, b"survive").unwrap();

    fs::create_dir_all(realm.path().join("links")).unwrap();
    fs::write(realm.path().join("links/real.md"), b"real").unwrap();
    // Link with a visible extension so it is enumerated by the walk.
    symlink(&target, realm.path().join("links/ptr.md")).unwrap();

    let out = run_in(realm.path(), &["rm", "links", "--json"]);
    assert_status(&out, 0);

    // The link is gone; the target it pointed at is untouched.
    assert!(!realm.path().join("links/ptr.md").exists());
    assert!(target.exists(), "symlink target must survive");
    assert_eq!(fs::read_to_string(&target).unwrap(), "survive");
    assert!(!realm.path().join("links").exists());
}

/// Empty directory is removed.
#[test]
fn removes_empty_directory_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(realm.path().join("empty")).unwrap();

    let out = run_in(realm.path(), &["rm", "empty", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["folders_removed"].as_array().unwrap().len(), 1);
    assert!(!realm.path().join("empty").exists());
}

/// Single-file `rm` JSON shape is unchanged (no directory fields leak).
#[test]
fn single_file_rm_json_shape_unchanged_via_cli() {
    let realm = TempDir::new().unwrap();
    fs::write(realm.path().join("a.md"), b"x").unwrap();

    let out = run_in(realm.path(), &["rm", "a.md", "--json"]);
    assert_status(&out, 0);

    let value: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
    assert_eq!(value["deleted"], "a.md");
    assert_eq!(value["existed"], true);
    assert!(
        value.get("is_directory").is_none(),
        "single-file rm must not carry directory fields"
    );
    assert!(!realm.path().join("a.md").exists());
}

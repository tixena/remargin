use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;

fn write_fixture(dir: &TempDir, name: &str, contents: &str) {
    fs::write(dir.path().join(name), contents).unwrap();
}

#[test]
fn get_json_returns_links_array() {
    let tmp = TempDir::new().unwrap();
    write_fixture(
        &tmp,
        "doc.md",
        "See [[Budget]] and [external](https://example.com/x).\nAlso [[Budget]] again.\n",
    );
    write_fixture(&tmp, "Budget.md", "---\ntitle: Q3 Budget\n---\n# Budget\n");

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .args(["get", "doc.md", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success(), "command failed: {output:?}");
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    // Content is unchanged (additive).
    assert!(payload["content"].as_str().unwrap().contains("[[Budget]]"));

    // Local links only: the external URL is dropped entirely.
    let links = payload["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "only the local link survives: {links:?}");
    assert!(
        links.iter().all(|l| l["target"] != "https://example.com/x"),
        "external URL must be absent: {links:?}"
    );

    let budget = links.iter().find(|l| l["target"] == "Budget").unwrap();
    assert_eq!(budget["path"], "Budget.md");
    assert_eq!(budget["title"], "Q3 Budget");
    assert_eq!(budget["count"], 2_i32);
    assert_eq!(budget["ref_lines"].as_array().unwrap().len(), 2);

    // No null keys: absent optionals are omitted, every link has a path.
    let budget_map = budget.as_object().unwrap();
    assert!(!budget_map.values().any(serde_json::Value::is_null));
    assert!(budget_map.contains_key("path"));
}

#[test]
fn get_json_drops_broken_internal_links() {
    let tmp = TempDir::new().unwrap();
    write_fixture(&tmp, "doc.md", "[[Exists]] and [[Missing]].\n");
    write_fixture(&tmp, "Exists.md", "# Exists\n");

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .args(["get", "doc.md", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success(), "command failed: {output:?}");
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let links = payload["links"].as_array().unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["target"], "Exists");
}

#[test]
fn get_pretty_renders_links_block() {
    let tmp = TempDir::new().unwrap();
    write_fixture(&tmp, "doc.md", "Link to [[Notes]] here.\n");
    write_fixture(&tmp, "Notes.md", "# Notes\n");

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .args(["get", "doc.md"])
        .output()
        .unwrap();

    assert!(output.status.success(), "command failed: {output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Link to [[Notes]] here."));
    assert!(
        stdout.contains("Links (1)"),
        "missing links block: {stdout}"
    );
    assert!(stdout.contains("Notes"));
    assert!(stdout.contains("Notes.md"));
}

#[test]
fn get_pretty_suppresses_block_at_zero_links() {
    let tmp = TempDir::new().unwrap();
    write_fixture(&tmp, "doc.md", "No links at all here.\n");

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .args(["get", "doc.md"])
        .output()
        .unwrap();

    assert!(output.status.success(), "command failed: {output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("Links ("),
        "block should be suppressed: {stdout}"
    );
}

#[test]
fn get_json_empty_links_when_none() {
    let tmp = TempDir::new().unwrap();
    write_fixture(&tmp, "doc.md", "Nothing to see here.\n");

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .args(["get", "doc.md", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success(), "command failed: {output:?}");
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["links"].as_array().unwrap().len(), 0);
}

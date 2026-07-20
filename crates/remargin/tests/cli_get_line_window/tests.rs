use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;

#[test]
fn get_lone_start_prints_tail_only() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("notes.md"), "one\ntwo\nthree\nfour\nfive\n").unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .args(["get", "notes.md", "--start", "3"])
        .output()
        .unwrap();

    assert!(output.status.success(), "command failed: {output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("three"),
        "tail must include line 3: {stdout}"
    );
    assert!(
        !stdout.contains("one") && !stdout.contains("two"),
        "lone --start must drop the head: {stdout}"
    );
}

#[test]
fn get_lone_end_prints_head_only() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("notes.md"), "one\ntwo\nthree\nfour\nfive\n").unwrap();

    let output = Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(tmp.path())
        .args(["get", "notes.md", "--end", "2"])
        .output()
        .unwrap();

    assert!(output.status.success(), "command failed: {output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("one") && stdout.contains("two"),
        "head must include lines 1-2: {stdout}"
    );
    assert!(
        !stdout.contains("three") && !stdout.contains("four") && !stdout.contains("five"),
        "lone --end must drop the tail: {stdout}"
    );
}

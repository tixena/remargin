//! rem-11u: `--config` must clap-conflict with `--identity`, `--type`,
//! and `--key`. Mixing "pass a config file" with "override pieces of
//! that file" silently misattributes comments (the class of bug that
//! produced rem-ce4). Clap rejects the combination at parse time
//! rather than letting the three-branch resolver see it.

#[cfg(test)]
mod tests {
    use core::str;

    use assert_cmd::Command;

    fn run(args: &[&str]) -> (Option<i32>, String) {
        let output = Command::cargo_bin("remargin")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from(str::from_utf8(&output.stderr).unwrap());
        (output.status.code(), stderr)
    }

    #[test]
    fn config_conflicts_with_identity() {
        let (code, stderr) = run(&[
            "--config",
            "/x.yaml",
            "--identity",
            "alice",
            "comment",
            "a.md",
            "hi",
        ]);
        assert_eq!(code, Some(2_i32));
        assert!(
            stderr.contains("cannot be used with '--identity") || stderr.contains("conflicts with"),
            "expected clap conflict for --identity, got: {stderr:?}"
        );
    }

    #[test]
    fn config_conflicts_with_type() {
        let (code, stderr) = run(&[
            "--config", "/x.yaml", "--type", "human", "comment", "a.md", "hi",
        ]);
        assert_eq!(code, Some(2_i32));
        assert!(
            stderr.contains("cannot be used with '--type") || stderr.contains("conflicts with"),
            "expected clap conflict for --type, got: {stderr:?}"
        );
    }

    #[test]
    fn config_conflicts_with_key() {
        let (code, stderr) = run(&[
            "--config", "/x.yaml", "--key", "id", "comment", "a.md", "hi",
        ]);
        assert_eq!(code, Some(2_i32));
        assert!(
            stderr.contains("cannot be used with '--key") || stderr.contains("conflicts with"),
            "expected clap conflict for --key, got: {stderr:?}"
        );
    }
}

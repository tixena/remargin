//! rem-11u / rem-zlx3: `--config` must clap-conflict with `--identity`,
//! `--type`, and `--key` on every identity-aware subcommand. Mixing a
//! whole-identity declaration with partial-identity flags produces the
//! "inherited-part-from-walk, replaced-part-from-flag" class of silent
//! misattribution (rem-ce4). Clap rejects the combination at parse time
//! rather than letting the three-branch resolver see it.
//!
//! Post-rem-zlx3 the identity group is per-subcommand (not global), so
//! the flags go AFTER the subcommand name. This file iterates over
//! every subcommand that flattens `IdentityArgs` and locks the conflict
//! in — regressing any of them would silently drop `--config` for that
//! subcommand (the exact bug that motivated this test file).
//!
//! The subcommand table below is intentionally exhaustive. If you add a
//! new subcommand with `IdentityArgs`, add it here too.

#[cfg(test)]
mod tests {
    use core::str;

    use assert_cmd::Command;

    /// Representative invocation for each identity-aware subcommand. The
    /// args after the subcommand are the minimum needed to get past
    /// clap's required-arg check so the `--config` vs `--identity/type/key`
    /// conflict surfaces. We don't execute the command; clap exits with
    /// code 2 before anything touches the filesystem.
    const SUBCOMMANDS: &[(&str, &[&str])] = &[
        ("ack", &["foo"]),
        ("batch", &["a.md", "--ops", "[]"]),
        ("comment", &["a.md", "hi"]),
        ("delete", &["a.md", "foo"]),
        ("edit", &["a.md", "foo", "content"]),
        ("mcp", &[]),
        ("migrate", &["a.md"]),
        ("plan", &["comment", "a.md", "hi"]),
        ("purge", &["a.md"]),
        ("react", &["a.md", "foo", "thumbsup"]),
        ("rm", &["a.md"]),
        ("sandbox", &["list"]),
        ("sign", &["a.md", "--ids", "foo"]),
        ("verify", &["a.md"]),
        ("write", &["a.md", "body"]),
    ];

    fn run(args: &[&str]) -> (Option<i32>, String) {
        let output = Command::cargo_bin("remargin")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from(str::from_utf8(&output.stderr).unwrap());
        (output.status.code(), stderr)
    }

    /// Identity flags go immediately after the parent subcommand name
    /// so they attach to the correct clap scope. `plan` flattens
    /// `IdentityArgs` on its parent (not per sub-action), so
    /// `remargin plan --config X comment a.md hi` is the valid shape;
    /// `remargin plan comment a.md hi --config X` would be interpreted
    /// as arguments to the `comment` sub-action, which does not accept
    /// `--config` here.
    fn args_for(cmd: &str, tail: &[&str], extra: &[&str]) -> Vec<String> {
        let mut out = vec![String::from(cmd)];
        out.extend(extra.iter().map(|s| String::from(*s)));
        out.extend(tail.iter().map(|s| String::from(*s)));
        out
    }

    fn run_strs(args: &[String]) -> (Option<i32>, String) {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run(&refs)
    }

    fn assert_conflict_strs(args: &[String], flag_label: &str) {
        let (code, stderr) = run_strs(args);
        assert_eq!(
            code,
            Some(2_i32),
            "expected clap exit code 2 for {args:?}, got {code:?}; stderr={stderr}"
        );
        assert!(
            stderr.contains(flag_label) || stderr.contains("conflicts with"),
            "expected clap conflict mentioning {flag_label:?}, got: {stderr}"
        );
    }

    #[test]
    fn config_conflicts_with_identity_on_every_subcommand() {
        for &(cmd, tail) in SUBCOMMANDS {
            let args = args_for(cmd, tail, &["--config", "/x.yaml", "--identity", "alice"]);
            assert_conflict_strs(&args, "--identity");
        }
    }

    #[test]
    fn config_conflicts_with_type_on_every_subcommand() {
        for &(cmd, tail) in SUBCOMMANDS {
            let args = args_for(cmd, tail, &["--config", "/x.yaml", "--type", "human"]);
            assert_conflict_strs(&args, "--type");
        }
    }

    #[test]
    fn config_conflicts_with_key_on_every_subcommand() {
        for &(cmd, tail) in SUBCOMMANDS {
            let args = args_for(cmd, tail, &["--config", "/x.yaml", "--key", "id"]);
            assert_conflict_strs(&args, "--key");
        }
    }
}

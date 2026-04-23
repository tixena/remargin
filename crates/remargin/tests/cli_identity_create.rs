//! `remargin identity create` integration tests (rem-8cnc).
//!
//! The subcommand prints an identity YAML block to stdout. These
//! tests cover the happy path, the `--key` branch, the `--json` shape,
//! and the round-trip invariant: the emitted YAML must load cleanly as
//! a `.remargin.yaml` (no `mode:` leak, no broken shape).

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::process::Output;

    use assert_cmd::Command;
    use serde_json::Value;
    use tempfile::TempDir;

    fn run(args: &[&str]) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .args(args)
            .output()
            .unwrap()
    }

    fn stdout_of(out: &Output) -> &str {
        str::from_utf8(&out.stdout).unwrap()
    }

    fn assert_ok(out: &Output) {
        assert!(
            out.status.success(),
            "remargin failed\nstdout: {}\nstderr: {}",
            str::from_utf8(&out.stdout).unwrap(),
            str::from_utf8(&out.stderr).unwrap(),
        );
    }

    #[test]
    fn create_without_key_emits_minimal_yaml() {
        let out = run(&[
            "identity",
            "create",
            "--identity",
            "alice",
            "--type",
            "human",
        ]);
        assert_ok(&out);
        let stdout = stdout_of(&out);
        assert_eq!(stdout, "identity: alice\ntype: human\n");
    }

    #[test]
    fn create_with_key_includes_key_line() {
        let out = run(&[
            "identity",
            "create",
            "--identity",
            "bot",
            "--type",
            "agent",
            "--key",
            "mykey",
        ]);
        assert_ok(&out);
        let stdout = stdout_of(&out);
        assert_eq!(stdout, "identity: bot\ntype: agent\nkey: mykey\n");
    }

    #[test]
    fn create_does_not_emit_mode_line() {
        // Mode is a tree property, not identity-scoped — never appears
        // in the emitted YAML (rem-8cnc).
        let out = run(&[
            "identity",
            "create",
            "--identity",
            "alice",
            "--type",
            "human",
        ]);
        assert_ok(&out);
        assert!(
            !stdout_of(&out).contains("mode:"),
            "identity create must not emit a mode: line"
        );
    }

    #[test]
    fn create_rejects_invalid_type() {
        let out = run(&[
            "identity",
            "create",
            "--identity",
            "alice",
            "--type",
            "martian",
        ]);
        assert!(!out.status.success(), "invalid --type must fail");
        // stdout stays clean so callers can redirect without capturing
        // garbage on failure.
        assert!(
            stdout_of(&out).is_empty(),
            "stdout must stay empty on --type error; got: {:?}",
            stdout_of(&out)
        );
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            stderr.contains("martian") || stderr.contains("invalid"),
            "stderr should mention the invalid type: {stderr}"
        );
    }

    #[test]
    fn create_json_mode_returns_structured_fields() {
        let out = run(&[
            "identity",
            "create",
            "--identity",
            "alice",
            "--type",
            "human",
            "--key",
            "mykey",
            "--json",
        ]);
        assert_ok(&out);
        let parsed: Value = serde_json::from_str(stdout_of(&out)).unwrap();
        assert_eq!(parsed["identity"].as_str().unwrap(), "alice");
        assert_eq!(parsed["type"].as_str().unwrap(), "human");
        assert_eq!(parsed["key"].as_str().unwrap(), "mykey");
    }

    #[test]
    fn create_output_round_trips_through_remargin_yaml() {
        // The emitted YAML must load cleanly as a `.remargin.yaml`.
        let tmp = TempDir::new().unwrap();
        let out = run(&[
            "identity",
            "create",
            "--identity",
            "alice",
            "--type",
            "human",
        ]);
        assert_ok(&out);
        fs::write(tmp.path().join(".remargin.yaml"), stdout_of(&out)).unwrap();

        // Invoke `identity show` inside the new realm — it must parse
        // the file we just wrote.
        let show = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args(["identity", "show", "--json"])
            .output()
            .unwrap();
        assert!(
            show.status.success(),
            "identity show failed after round-trip:\nstderr: {}",
            str::from_utf8(&show.stderr).unwrap()
        );
        let parsed: Value = serde_json::from_str(str::from_utf8(&show.stdout).unwrap()).unwrap();
        assert!(parsed["found"].as_bool().unwrap());
        assert_eq!(parsed["identity"].as_str().unwrap(), "alice");
        assert_eq!(parsed["author_type"].as_str().unwrap(), "human");
    }

    #[test]
    fn bare_identity_still_works_backward_compat() {
        // Pre-rem-8cnc callers invoked `remargin identity` (no
        // subcommand) to resolve the active identity. That surface must
        // keep working — `identity` without a subcommand defaults to
        // the show action.
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join(".remargin.yaml"),
            "identity: bob\ntype: human\n",
        )
        .unwrap();

        let out = Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .args(["identity", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let parsed: Value = serde_json::from_str(str::from_utf8(&out.stdout).unwrap()).unwrap();
        assert_eq!(parsed["identity"].as_str().unwrap(), "bob");
    }

    #[test]
    fn create_requires_identity_arg() {
        let out = run(&["identity", "create", "--type", "human"]);
        assert!(!out.status.success(), "--identity required");
    }

    #[test]
    fn create_requires_type_arg() {
        let out = run(&["identity", "create", "--identity", "alice"]);
        assert!(!out.status.success(), "--type required");
    }

    #[test]
    fn stdout_has_no_extra_stderr_noise() {
        // A user who does `remargin identity create ... > .remargin.yaml`
        // deserves a quiet stderr so the YAML stays pristine.
        let out = run(&[
            "identity",
            "create",
            "--identity",
            "alice",
            "--type",
            "human",
        ]);
        assert_ok(&out);
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            stderr.is_empty(),
            "stderr must be empty on success; got: {stderr:?}"
        );
    }
}

//! `remargin lint` end-to-end coverage for the rem-welo
//! permissions-aware op-name validation.
//!
//! - A typo in `permissions.deny_ops.ops` (`purg`) inside an ambient
//!   `.remargin.yaml` causes `remargin lint <doc>` to exit non-zero
//!   with an error message that names the offending typo and lists
//!   the valid op names.
//! - A clean `.remargin.yaml` does NOT make the doc fail to lint.
//! - `--json` mirrors the same payload through the documented schema.

#[cfg(test)]
mod tests {
    use core::str;
    use std::fs;
    use std::path::Path;
    use std::process::Output;

    use assert_cmd::Command;
    use serde_json::Value;
    use tempfile::TempDir;

    const CLEAN_DOC: &str = "# Title\n\nbody\n";

    fn run_in(dir: &Path, args: &[&str]) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap()
    }

    fn write_realm_yaml(realm: &Path, body: &str) {
        fs::write(realm.join(".remargin.yaml"), body).unwrap();
    }

    /// Acceptance criterion 1 + 2: a typo in `deny_ops.ops` causes
    /// `remargin lint` to exit non-zero with an error that names the
    /// typo AND lists the valid ops. The doc sits in a sub-realm so
    /// the lint command's own parent walk surfaces the finding rather
    /// than the cwd-level config preflight.
    #[test]
    fn lint_flags_unknown_op_in_deny_ops() {
        let workspace = TempDir::new().unwrap();
        let realm = workspace.path().join("realm");
        let docdir = realm.join("docs");
        fs::create_dir_all(&docdir).unwrap();
        write_realm_yaml(
            &realm,
            "permissions:\n  deny_ops:\n    - path: src\n      ops: [purg, delete]\n",
        );
        fs::write(docdir.join("doc.md"), CLEAN_DOC).unwrap();

        let out = run_in(workspace.path(), &["lint", "realm/docs/doc.md"]);
        assert!(
            !out.status.success(),
            "lint should fail; stdout={}, stderr={}",
            str::from_utf8(&out.stdout).unwrap(),
            str::from_utf8(&out.stderr).unwrap(),
        );
        let stderr = str::from_utf8(&out.stderr).unwrap();
        assert!(
            stderr.contains("purg"),
            "stderr did not name typo: {stderr}"
        );
        // Valid-ops list is rendered by serde_yaml's "expected one of …".
        assert!(
            stderr.contains("purge") && stderr.contains("delete"),
            "stderr did not list valid ops: {stderr}"
        );
        // Source file is named so the user knows where to fix.
        assert!(
            stderr.contains(".remargin.yaml"),
            "stderr did not name source file: {stderr}"
        );
    }

    /// `--json` surfaces the same finding in the canonical
    /// `{ errors, ok, permissions }` payload. The doc lives in a
    /// nested directory so the `lint`-time parent walk picks up the
    /// realm config without the cwd-level config preflight short-
    /// circuiting first.
    #[test]
    fn lint_json_includes_permissions_finding() {
        let workspace = TempDir::new().unwrap();
        let realm = workspace.path().join("realm");
        let docdir = realm.join("docs");
        fs::create_dir_all(&docdir).unwrap();
        write_realm_yaml(
            &realm,
            "permissions:\n  deny_ops:\n    - path: src\n      ops: [purg]\n",
        );
        fs::write(docdir.join("doc.md"), CLEAN_DOC).unwrap();

        // Run from the workspace root (no `.remargin.yaml` here) so
        // the lint command itself drives the parent walk.
        let out = run_in(workspace.path(), &["lint", "--json", "realm/docs/doc.md"]);
        assert!(
            !out.status.success(),
            "lint should fail; stdout={}, stderr={}",
            str::from_utf8(&out.stdout).unwrap(),
            str::from_utf8(&out.stderr).unwrap(),
        );
        let stdout = str::from_utf8(&out.stdout).unwrap();
        let value: Value = serde_json::from_str(stdout).unwrap();
        assert_eq!(value.get("ok").and_then(Value::as_bool), Some(false));
        let permissions = value.get("permissions").and_then(Value::as_array).unwrap();
        assert_eq!(permissions.len(), 1);
        let entry = &permissions[0];
        let message = entry.get("message").and_then(Value::as_str).unwrap();
        assert!(message.contains("purg"), "message: {message}");
        assert!(
            entry.get("source_file").and_then(Value::as_str).is_some(),
            "source_file present"
        );
    }

    /// Existing valid configs (`[purge, delete]`) keep working — no
    /// permissions findings, exit zero.
    #[test]
    fn lint_clean_config_passes() {
        let workspace = TempDir::new().unwrap();
        let realm = workspace.path().join("realm");
        let docdir = realm.join("docs");
        fs::create_dir_all(&docdir).unwrap();
        write_realm_yaml(
            &realm,
            "permissions:\n  deny_ops:\n    - path: src\n      ops: [purge, delete]\n",
        );
        fs::write(docdir.join("doc.md"), CLEAN_DOC).unwrap();

        let out = run_in(workspace.path(), &["lint", "realm/docs/doc.md"]);
        assert!(
            out.status.success(),
            "lint should pass; stderr={}",
            str::from_utf8(&out.stderr).unwrap(),
        );
    }
}

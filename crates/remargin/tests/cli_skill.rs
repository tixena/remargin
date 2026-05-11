//! `remargin skill {install,test,uninstall}` integration smoke tests.

#[cfg(test)]
mod tests {
    use core::str;
    use std::process::Output;

    use assert_cmd::Command;
    use tempfile::TempDir;

    fn run(tmp: &TempDir, args: &[&str]) -> Output {
        Command::cargo_bin("remargin")
            .unwrap()
            .current_dir(tmp.path())
            .env("HOME", tmp.path())
            .args(args)
            .output()
            .unwrap()
    }

    #[test]
    fn skill_install_test_uninstall_round_trip() {
        let tmp = TempDir::new().unwrap();

        let install = run(&tmp, &["skill", "install"]);
        assert!(install.status.success(), "install failed: {install:?}");
        assert!(
            tmp.path().join(".claude/skills/remargin/SKILL.md").exists(),
            "SKILL.md not written"
        );

        let test = run(&tmp, &["skill", "test"]);
        assert!(test.status.success(), "test failed: {test:?}");

        let uninstall = run(&tmp, &["skill", "uninstall"]);
        assert!(
            uninstall.status.success(),
            "uninstall failed: {uninstall:?}"
        );
        assert!(
            !tmp.path().join(".claude/skills/remargin").exists(),
            "skill dir not removed"
        );
    }

    #[test]
    fn skill_test_when_not_installed_reports_status() {
        let tmp = TempDir::new().unwrap();
        let output = run(&tmp, &["skill", "test"]);
        let stderr = str::from_utf8(&output.stderr).unwrap();
        let stdout = str::from_utf8(&output.stdout).unwrap();
        let combined = format!("{stdout}{stderr}");
        assert!(
            combined.contains("not_installed")
                || combined.contains("not installed")
                || combined.contains("NotInstalled"),
            "expected not-installed diagnostic; got stdout={stdout:?} stderr={stderr:?}"
        );
    }
}

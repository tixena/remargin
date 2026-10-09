//! `remargin goose mcp` lifecycle, `test` and `doctor` runs against a temp home.

use core::str;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::cargo::CommandCargoExt as _;
use serde_json::Value;
use serde_yaml::Value as Yaml;
use tempfile::TempDir;

/// Run a lifecycle subcommand with `$HOME` pinned and `XDG_CONFIG_HOME`
/// cleared, so the user scope resolves to the temp home's `.config` rather
/// than the developer's real goose config.
fn run_lifecycle(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::cargo_bin("remargin")
        .unwrap()
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .args(args)
        .output()
        .unwrap()
}

fn stdout_of(out: &Output) -> &str {
    str::from_utf8(&out.stdout).unwrap()
}

fn stderr_of(out: &Output) -> &str {
    str::from_utf8(&out.stderr).unwrap()
}

fn assert_status(out: &Output, expected: i32) {
    assert_eq!(
        out.status.code(),
        Some(expected),
        "remargin exited with {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        stdout_of(out),
        stderr_of(out),
    );
}

fn report_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn status_of(out: &Output) -> String {
    report_of(out)["status"].as_str().unwrap().to_owned()
}

fn user_config(home: &Path) -> PathBuf {
    home.join(".config/goose/config.yaml")
}

fn config_yaml(path: &Path) -> Yaml {
    serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn entry_at(path: &Path) -> Yaml {
    config_yaml(path)
        .get("extensions")
        .unwrap()
        .get("remargin")
        .unwrap()
        .clone()
}

/// A config carrying goose's own provider settings and an unrelated
/// extension — the state any real machine is in before remargin arrives.
fn seed_populated_config(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        "GOOSE_TELEMETRY_ENABLED: false\nactive_provider: ollama\nproviders:\n  ollama:\n    \
         enabled: true\n    model: gemma4:31b\nextensions:\n  developer:\n    enabled: true\n    \
         type: builtin\n    name: developer\n",
    )
    .unwrap();
}

/// The same config written in flow style — a shape the in-place editor
/// declines, so a write of it re-serializes the whole document.
fn seed_flow_style_config(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        "active_provider: ollama\nextensions: {developer: {enabled: true, type: builtin}}\n",
    )
    .unwrap();
}

/// `uninstall` removes exactly what `install` wrote; siblings and provider settings survive both.
#[test]
fn install_then_uninstall_round_trips_and_preserves_the_rest_of_the_config() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    let config = user_config(home.path());
    seed_populated_config(&config);

    let installed = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "install"],
    );
    assert_status(&installed, 0);
    assert_eq!(status_of(&installed), "installed");
    assert_eq!(
        report_of(&installed)["config_file"],
        Value::from(config.display().to_string()),
    );

    let again = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "install"],
    );
    assert_eq!(status_of(&again), "already_installed");

    let removed = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "uninstall"],
    );
    assert_eq!(status_of(&removed), "uninstalled");

    let after = config_yaml(&config);
    assert_eq!(after.get("active_provider").unwrap(), &Yaml::from("ollama"));
    assert!(
        after.get("providers").is_some(),
        "providers dropped: {after:?}"
    );
    let extensions = after.get("extensions").unwrap();
    assert!(
        extensions.get("developer").is_some(),
        "sibling extension dropped: {extensions:?}",
    );
    assert!(
        extensions.get("remargin").is_none(),
        "entry survived removal"
    );
}

/// goose namespaces tools as `<name>__<tool>`, and warns and continues past a `cmd` it cannot spawn.
#[test]
fn generated_entry_pins_the_name_and_an_absolute_binary() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    run_lifecycle(home.path(), realm.path(), &["goose", "mcp", "install"]);

    let entry = entry_at(&user_config(home.path()));
    assert_eq!(entry.get("name").unwrap(), &Yaml::from("remargin"));
    assert_eq!(entry.get("type").unwrap(), &Yaml::from("stdio"));
    assert_eq!(entry.get("enabled").unwrap(), &Yaml::Bool(true));
    assert_eq!(
        entry.get("args").unwrap(),
        &Yaml::Sequence(vec![Yaml::from("mcp")]),
    );

    let command = entry.get("cmd").unwrap().as_str().unwrap().to_owned();
    assert!(
        Path::new(&command).is_absolute(),
        "entry command must be absolute: {command}",
    );
    assert!(Path::new(&command).is_file(), "entry command must exist");
}

/// goose finds no project config on its own, so `--local` names `GOOSE_ADDITIONAL_CONFIG_FILES`.
#[test]
fn local_install_targets_the_project_file_and_names_the_env_var() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();

    let out = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "install", "--local"],
    );
    assert_status(&out, 0);
    let report = report_of(&out);
    assert_eq!(report["scope"], Value::from("project"));
    assert_eq!(
        report["requires_env"],
        Value::from("GOOSE_ADDITIONAL_CONFIG_FILES"),
    );

    let local = realm.path().join(".goose/config.yaml");
    assert!(local.is_file(), "project config missing");
    assert!(
        !user_config(home.path()).exists(),
        "user scope must be untouched",
    );

    let text = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "install", "--local"],
    );
    assert!(
        stderr_of(&text).contains("GOOSE_ADDITIONAL_CONFIG_FILES"),
        "the env-var requirement must be stated: {}",
        stderr_of(&text),
    );
}

/// The user config is the one goose reads on its own, so the install carries no such requirement.
#[test]
fn user_install_reports_no_env_requirement() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    let out = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "install"],
    );
    assert_eq!(report_of(&out)["scope"], Value::from("user"));
    assert_eq!(report_of(&out)["requires_env"], Value::Null);
}

/// goose reads its provider from this file, so an unparseable config is refused, never rewritten.
#[test]
fn install_refuses_to_overwrite_an_unparseable_config() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    let config = user_config(home.path());
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "extensions: [ this is not\n").unwrap();

    let out = run_lifecycle(home.path(), realm.path(), &["goose", "mcp", "install"]);
    assert_ne!(
        out.status.code(),
        Some(0_i32),
        "a torn config must not succeed"
    );
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "extensions: [ this is not\n",
        "the unreadable config must survive untouched",
    );
}

/// A flow-style config is re-serialized, losing comments and spacing, and the write says so.
#[test]
fn install_warns_when_the_write_normalizes_the_config_layout() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    let config = user_config(home.path());

    seed_flow_style_config(&config);
    let reported = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "install"],
    );
    assert_status(&reported, 0);
    assert_eq!(status_of(&reported), "installed");
    assert_eq!(report_of(&reported)["normalized_layout"], Value::Bool(true));

    seed_flow_style_config(&config);
    let text = run_lifecycle(home.path(), realm.path(), &["goose", "mcp", "install"]);
    let stderr = stderr_of(&text);
    assert!(
        stderr.contains("warning") && stderr.contains(&config.display().to_string()),
        "the normalizing write must warn and name the file: {stderr}",
    );

    let after = config_yaml(&config);
    assert_eq!(after.get("active_provider").unwrap(), &Yaml::from("ollama"));
    assert!(
        after.get("extensions").unwrap().get("developer").is_some(),
        "sibling extension dropped: {after:?}",
    );
    assert_eq!(
        entry_at(&config).get("name").unwrap(),
        &Yaml::from("remargin"),
    );
}

/// An in-place edit keeps the layout, so no warning; neither does a run that writes nothing.
#[test]
fn install_stays_quiet_when_the_write_preserves_the_layout() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    let config = user_config(home.path());
    seed_populated_config(&config);

    let written = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "install"],
    );
    assert_eq!(status_of(&written), "installed");
    assert_eq!(report_of(&written)["normalized_layout"], Value::Bool(false));

    seed_populated_config(&config);
    let text = run_lifecycle(home.path(), realm.path(), &["goose", "mcp", "install"]);
    assert!(
        !stderr_of(&text).contains("warning"),
        "a layout-preserving write must not warn: {}",
        stderr_of(&text),
    );

    let again = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "install"],
    );
    assert_eq!(status_of(&again), "already_installed");
    assert_eq!(report_of(&again)["normalized_layout"], Value::Bool(false));

    let removed = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "uninstall"],
    );
    assert_eq!(status_of(&removed), "uninstalled");
    assert_eq!(report_of(&removed)["normalized_layout"], Value::Bool(false));

    let absent = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "uninstall"],
    );
    assert_eq!(status_of(&absent), "not_installed");
    assert_eq!(report_of(&absent)["normalized_layout"], Value::Bool(false));
}

/// The three verdicts `test` distinguishes: registered, absent, and present but a dead end.
#[test]
fn test_subcommand_reports_installed_absent_and_broken() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();

    let absent = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "test"],
    );
    assert_eq!(status_of(&absent), "not_installed");

    run_lifecycle(home.path(), realm.path(), &["goose", "mcp", "install"]);
    let wired = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "test"],
    );
    assert_eq!(status_of(&wired), "installed");

    // goose warns about an extension it cannot start and opens the session without its tools.
    let config = user_config(home.path());
    fs::write(
        &config,
        "extensions:\n  remargin:\n    enabled: true\n    type: stdio\n    name: remargin\n    \
         cmd: /nonexistent/remargin\n    args:\n    - mcp\n",
    )
    .unwrap();
    let broken = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "test"],
    );
    assert_eq!(status_of(&broken), "broken");
    let detail = report_of(&broken)["detail"].as_str().unwrap().to_owned();
    assert!(
        detail.contains("/nonexistent/remargin"),
        "broken detail should name the missing binary: {detail}",
    );

    // A renamed entry still loads, but its tools arrive under a prefix the guard does not redirect to.
    fs::write(
        &config,
        "extensions:\n  remargin:\n    enabled: true\n    type: stdio\n    name: notremargin\n    \
         cmd: /bin/sh\n    args:\n    - mcp\n",
    )
    .unwrap();
    let renamed = run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "mcp", "--json", "test"],
    );
    assert_eq!(status_of(&renamed), "broken");
    assert!(
        report_of(&renamed)["detail"]
            .as_str()
            .unwrap()
            .contains("notremargin__"),
        "broken detail should name the wrong tool prefix",
    );
}

/// A wired guard with no extension redirects to ops the session never received; `doctor` flags it.
#[test]
fn doctor_flags_a_wired_guard_without_the_extension() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".agents/plugins")).unwrap();
    run_lifecycle(home.path(), realm.path(), &["claude", "pretool", "install"]);
    run_lifecycle(
        home.path(),
        realm.path(),
        &["claude", "session-guard", "install"],
    );
    run_lifecycle(home.path(), realm.path(), &["goose", "pretool", "install"]);
    run_lifecycle(
        home.path(),
        realm.path(),
        &["goose", "session-guard", "install"],
    );

    let user_settings = home.path().join(".claude/settings.json");
    let args = [
        "doctor",
        "--user-settings",
        user_settings.to_str().unwrap(),
        "--check=goose-mcp",
        "--json",
    ];

    let out = run_lifecycle(home.path(), realm.path(), &args);
    assert_status(&out, 1);
    let kinds: Vec<String> = report_of(&out)["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, vec![String::from("goose_mcp_missing")]);

    run_lifecycle(home.path(), realm.path(), &["goose", "mcp", "install"]);
    let clean = run_lifecycle(home.path(), realm.path(), &args);
    assert_status(&clean, 0);
}

/// With no guard there is no redirect yet; `goose-guard` owns that repair and comes first.
#[test]
fn doctor_stays_quiet_about_the_extension_when_the_guard_is_absent() {
    let home = TempDir::new().unwrap();
    let realm = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".agents/plugins")).unwrap();
    run_lifecycle(home.path(), realm.path(), &["claude", "pretool", "install"]);
    run_lifecycle(
        home.path(),
        realm.path(),
        &["claude", "session-guard", "install"],
    );

    let user_settings = home.path().join(".claude/settings.json");
    let out = run_lifecycle(
        home.path(),
        realm.path(),
        &[
            "doctor",
            "--user-settings",
            user_settings.to_str().unwrap(),
            "--check=goose-mcp",
            "--json",
        ],
    );
    assert_status(&out, 0);
}

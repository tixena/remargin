//! Unit tests for `permissions::goose_mcp_install`: install into an empty config, install
//! beside unrelated extensions, uninstall, and the `test` verdicts.
//!
//! The fixtures are the entry shape read off a live goose 1.45.0 config,
//! and every "broken" case is a failure mode observed on that goose: a
//! `cmd` that does not exist, an entry with no `cmd` at all, and an entry
//! turned off with `enabled: false` all leave the session with zero
//! remargin tools while goose starts happily.

use std::path::{Path, PathBuf};

use os_shim::System;
use os_shim::mock::MemorySystem;
use serde_yaml::{Mapping, Value};

use super::{
    EXTENSION_KEY, EXTENSION_NAME, InstallOutcome, TestOutcome, UninstallOutcome, install,
    local_config_file, test, uninstall, user_config_file,
};

const EXE: &str = "/opt/bin/remargin";

/// A hand-kept config: own-line and trailing comments, blank-line grouping, both quoting styles.
const COMMENTED_CONFIG: &[&str] = &[
    "# goose config -- hand maintained",
    "GOOSE_TELEMETRY_ENABLED: false",
    "",
    "# the provider comes first, everything else after",
    "active_provider: \"ollama\"",
    "providers:",
    "  ollama:",
    "    enabled: true # this one stays on",
    "    model: 'gemma4:31b'",
    "",
    "extensions:",
    "  developer:",
    "    enabled: true",
    "    type: builtin",
    "    name: developer",
    "",
    "# nothing below here is goose's",
    "notes: see the wiki",
];

/// A remargin entry that has drifted: an old `cmd`, a comment of its own, a sibling after it.
const DRIFTED_CONFIG: &[&str] = &[
    "# goose config -- hand maintained",
    "active_provider: ollama",
    "",
    "extensions:",
    "  remargin:",
    "    # points at the binary an older install put here",
    "    type: stdio",
    "    name: remargin",
    "    cmd: /old/remargin",
    "    args:",
    "    - mcp",
    "  developer:",
    "    enabled: true",
    "",
    "# tail comment",
    "GOOSE_TELEMETRY_ENABLED: false",
];

const SOLE_ENTRY_CONFIG: &[&str] = &[
    "# goose config -- hand maintained",
    "active_provider: ollama",
    "",
    "extensions:",
    "  remargin:",
    "    enabled: true",
    "    type: stdio",
    "    name: remargin",
    "    cmd: /opt/bin/remargin",
    "    args:",
    "    - mcp",
    "",
    "# tail comment",
    "GOOSE_TELEMETRY_ENABLED: false",
];

/// [`SOLE_ENTRY_CONFIG`] after an uninstall: `extensions` is an empty mapping, not a bare key.
const SOLE_ENTRY_UNINSTALLED: &[&str] = &[
    "# goose config -- hand maintained",
    "active_provider: ollama",
    "",
    "extensions: {}",
    "",
    "# tail comment",
    "GOOSE_TELEMETRY_ENABLED: false",
];

fn home() -> PathBuf {
    PathBuf::from("/home/u")
}

fn config_path() -> PathBuf {
    PathBuf::from("/home/u/.config/goose/config.yaml")
}

/// A mock whose `current_exe` is the binary the installer must embed.
fn mock() -> MemorySystem {
    MemorySystem::new()
        .with_current_exe(Path::new(EXE))
        .unwrap()
        .with_file(Path::new(EXE), b"binary")
        .unwrap()
}

fn seed(system: MemorySystem, body: &str) -> MemorySystem {
    system.with_file(config_path(), body.as_bytes()).unwrap()
}

fn joined(lines: &[&str]) -> String {
    let mut body = String::new();
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    body
}

/// `body` with remargin's entry block dropped — its key line and every
/// line indented under it — so a test can compare every byte a write was
/// supposed to leave alone.
fn without_entry_block(body: &str) -> String {
    let mut kept = String::new();
    let mut inside = false;
    for line in body.split_inclusive('\n') {
        if inside && line.starts_with("    ") && !line.trim().is_empty() {
            continue;
        }
        inside = line.trim_end() == "  remargin:";
        if !inside {
            kept.push_str(line);
        }
    }
    kept
}

fn config_of(system: &dyn System) -> Mapping {
    serde_yaml::from_str::<Value>(&system.read_to_string(&config_path()).unwrap())
        .unwrap()
        .as_mapping()
        .unwrap()
        .clone()
}

fn entry_of(system: &dyn System) -> Mapping {
    config_of(system)
        .get(Value::from("extensions"))
        .unwrap()
        .as_mapping()
        .unwrap()
        .get(Value::from(EXTENSION_KEY))
        .unwrap()
        .as_mapping()
        .unwrap()
        .clone()
}

fn field(entry: &Mapping, key: &str) -> Value {
    entry.get(Value::from(key)).cloned().unwrap()
}

fn installed(normalized_layout: bool) -> InstallOutcome {
    InstallOutcome::Installed { normalized_layout }
}

fn uninstalled(normalized_layout: bool) -> UninstallOutcome {
    UninstallOutcome::Uninstalled { normalized_layout }
}

fn expect_broken(outcome: TestOutcome) -> String {
    assert!(
        matches!(outcome, TestOutcome::Broken(_)),
        "expected Broken, got {outcome:?}",
    );
    let TestOutcome::Broken(reason) = outcome else {
        return String::new();
    };
    reason
}

/// A hand-written entry in the shape goose accepts, parameterized so each
/// broken-shape test can bend exactly one field.
fn entry_yaml(fields: &str) -> String {
    format!("extensions:\n  remargin:\n{fields}")
}

fn wired_yaml() -> String {
    entry_yaml(
        "    enabled: true\n    type: stdio\n    name: remargin\n    description: managed \
         markdown\n    cmd: /opt/bin/remargin\n    args:\n    - mcp\n    timeout: 300\n",
    )
}

#[test]
fn user_config_file_falls_back_to_dot_config() {
    assert_eq!(user_config_file(&mock(), &home()), config_path());
}

#[test]
fn user_config_file_follows_xdg_config_home() {
    let system = mock().with_env("XDG_CONFIG_HOME", "/xdg").unwrap();
    assert_eq!(
        user_config_file(&system, &home()),
        PathBuf::from("/xdg/goose/config.yaml"),
    );
}

/// Treating an empty `XDG_CONFIG_HOME` as a config home would point at `/goose/config.yaml`.
#[test]
fn user_config_file_ignores_an_empty_xdg_config_home() {
    let system = mock().with_env("XDG_CONFIG_HOME", "").unwrap();
    assert_eq!(user_config_file(&system, &home()), config_path());
}

#[test]
fn local_config_file_lands_under_the_project_goose_dir() {
    assert_eq!(
        local_config_file(Path::new("/w/repo")),
        PathBuf::from("/w/repo/.goose/config.yaml"),
    );
}

#[test]
fn install_writes_the_entry_when_no_config_exists() {
    let system = mock();
    assert_eq!(install(&system, &config_path()).unwrap(), installed(false));

    let entry = entry_of(&system);
    assert_eq!(field(&entry, "type"), Value::from("stdio"));
    assert_eq!(field(&entry, "enabled"), Value::Bool(true));
    assert_eq!(
        field(&entry, "args"),
        Value::Sequence(vec![Value::from("mcp")]),
    );
    assert_eq!(field(&entry, "timeout"), Value::from(300_u64));
}

/// goose builds tool names from `name`, not from the entry's key.
#[test]
fn generated_entry_pins_the_name_the_tool_prefix_comes_from() {
    let system = mock();
    let _outcome = install(&system, &config_path()).unwrap();
    assert_eq!(
        entry_of(&system).get(Value::from("name")),
        Some(&Value::from(EXTENSION_NAME)),
    );
}

/// goose warns and continues when it cannot spawn an extension, so the path must be absolute.
#[test]
fn generated_entry_names_the_binary_by_absolute_path() {
    let system = mock();
    let _outcome = install(&system, &config_path()).unwrap();
    let entry = entry_of(&system);
    let command = field(&entry, "cmd");
    assert_eq!(command, Value::from(EXE));
    assert!(Path::new(command.as_str().unwrap()).is_absolute());
}

#[test]
fn install_is_idempotent() {
    let system = mock();
    assert_eq!(install(&system, &config_path()).unwrap(), installed(false));
    assert_eq!(
        install(&system, &config_path()).unwrap(),
        InstallOutcome::AlreadyInstalled,
    );
}

#[test]
fn reinstalling_leaves_the_file_byte_identical() {
    let system = mock();
    let _outcome = install(&system, &config_path()).unwrap();
    let before = system.read_to_string(&config_path()).unwrap();
    assert_eq!(
        install(&system, &config_path()).unwrap(),
        InstallOutcome::AlreadyInstalled,
    );
    assert_eq!(system.read_to_string(&config_path()).unwrap(), before);
}

#[test]
fn install_rewrites_a_drifted_entry_in_place() {
    let system = seed(
        mock(),
        &entry_yaml("    type: stdio\n    name: remargin\n    cmd: /old/remargin\n"),
    );
    assert_eq!(install(&system, &config_path()).unwrap(), installed(false));
    assert_eq!(field(&entry_of(&system), "cmd"), Value::from(EXE));
}

/// goose reads its provider, its model and every other extension from this same file.
#[test]
fn install_preserves_sibling_extensions_and_unrelated_keys() {
    let system = seed(
        mock(),
        "GOOSE_TELEMETRY_ENABLED: false\nactive_provider: ollama\nproviders:\n  ollama:\n    \
         enabled: true\n    model: gemma4:31b\nextensions:\n  developer:\n    enabled: true\n    \
         type: builtin\n    name: developer\n",
    );
    let _outcome = install(&system, &config_path()).unwrap();

    let config = config_of(&system);
    assert_eq!(
        config.get(Value::from("active_provider")),
        Some(&Value::from("ollama")),
    );
    assert_eq!(
        config.get(Value::from("GOOSE_TELEMETRY_ENABLED")),
        Some(&Value::Bool(false)),
    );
    assert!(config.get(Value::from("providers")).is_some());

    let extensions = config
        .get(Value::from("extensions"))
        .unwrap()
        .as_mapping()
        .unwrap();
    assert!(
        extensions.get(Value::from("developer")).is_some(),
        "sibling extension dropped: {extensions:?}",
    );
    assert!(extensions.get(Value::from(EXTENSION_KEY)).is_some());
}

/// An unparseable config is refused, not rewritten: a rewrite would cost the whole goose setup.
#[test]
fn install_refuses_to_overwrite_an_unparseable_config() {
    let system = seed(mock(), "extensions: [ this is not\n");
    let err = install(&system, &config_path()).unwrap_err();
    assert!(
        err.to_string().contains("not valid YAML"),
        "error should name the parse fault: {err}",
    );
    assert_eq!(
        system.read_to_string(&config_path()).unwrap(),
        "extensions: [ this is not\n",
        "the unreadable config must survive untouched",
    );
}

/// goose leaves an empty `config.yaml` before its first `configure`; it parses as null.
#[test]
fn install_treats_an_empty_config_as_an_absence_to_fill() {
    let system = seed(mock(), "");
    assert_eq!(install(&system, &config_path()).unwrap(), installed(false));
    assert_eq!(
        entry_of(&system).get(Value::from("name")),
        Some(&Value::from(EXTENSION_NAME)),
    );
}

#[test]
fn uninstall_removes_only_remargins_entry() {
    let system = seed(
        mock(),
        "active_provider: ollama\nextensions:\n  developer:\n    enabled: true\n    type: \
         builtin\n",
    );
    let _outcome = install(&system, &config_path()).unwrap();
    assert_eq!(
        uninstall(&system, &config_path()).unwrap(),
        uninstalled(false),
    );

    let config = config_of(&system);
    assert_eq!(
        config.get(Value::from("active_provider")),
        Some(&Value::from("ollama")),
    );
    let extensions = config
        .get(Value::from("extensions"))
        .unwrap()
        .as_mapping()
        .unwrap();
    assert!(extensions.get(Value::from("developer")).is_some());
    assert!(extensions.get(Value::from(EXTENSION_KEY)).is_none());
}

#[test]
fn uninstall_is_a_no_op_when_absent() {
    let system = mock();
    assert_eq!(
        uninstall(&system, &config_path()).unwrap(),
        UninstallOutcome::NotInstalled,
    );

    let seeded = seed(mock(), "active_provider: ollama\n");
    assert_eq!(
        uninstall(&seeded, &config_path()).unwrap(),
        UninstallOutcome::NotInstalled,
    );
    assert_eq!(
        seeded.read_to_string(&config_path()).unwrap(),
        "active_provider: ollama\n",
        "a config with no remargin entry must not be rewritten",
    );
}

#[test]
fn uninstall_refuses_to_rewrite_an_unparseable_config() {
    let system = seed(mock(), "extensions: [ this is not\n");
    let err = uninstall(&system, &config_path()).unwrap_err();
    assert!(
        err.to_string().contains("not valid YAML"),
        "error should name the parse fault: {err}",
    );
}

#[test]
fn test_reports_installed_when_wired() {
    let system = mock();
    let _outcome = install(&system, &config_path()).unwrap();
    assert_eq!(
        test(&system, &config_path()).unwrap(),
        TestOutcome::Installed
    );
}

#[test]
fn test_reports_not_installed_when_absent() {
    assert_eq!(
        test(&mock(), &config_path()).unwrap(),
        TestOutcome::NotInstalled,
    );
    let seeded = seed(
        mock(),
        "active_provider: ollama\nextensions:\n  developer:\n    x: 1\n",
    );
    assert_eq!(
        test(&seeded, &config_path()).unwrap(),
        TestOutcome::NotInstalled,
    );
}

/// goose prints a warning and starts a session with no remargin tools.
#[test]
fn test_reports_broken_when_the_binary_is_gone() {
    let system = MemorySystem::new()
        .with_current_exe(Path::new(EXE))
        .unwrap()
        .with_file(config_path(), wired_yaml().as_bytes())
        .unwrap();
    let reason = expect_broken(test(&system, &config_path()).unwrap());
    assert!(
        reason.contains("/opt/bin/remargin"),
        "reason should name the missing binary: {reason}",
    );
}

#[test]
fn test_reports_broken_for_each_dead_end_shape() {
    let cases = [
        (
            "    enabled: false\n    type: stdio\n    name: remargin\n    cmd: \
             /opt/bin/remargin\n    args:\n    - mcp\n",
            "enabled: false",
        ),
        (
            "    enabled: true\n    type: stdio\n    name: remargin\n    args:\n    - mcp\n",
            "no `cmd`",
        ),
        (
            "    enabled: true\n    type: stdio\n    name: notremargin\n    cmd: \
             /opt/bin/remargin\n    args:\n    - mcp\n",
            "notremargin__",
        ),
        (
            "    enabled: true\n    type: builtin\n    name: remargin\n    cmd: \
             /opt/bin/remargin\n    args:\n    - mcp\n",
            "not `stdio`",
        ),
        (
            "    enabled: true\n    type: stdio\n    name: remargin\n    cmd: \
             /opt/bin/remargin\n    args: []\n",
            "does not pass `mcp`",
        ),
    ];
    for (fields, expected) in cases {
        let system = seed(mock(), &entry_yaml(fields));
        let reason = expect_broken(test(&system, &config_path()).unwrap());
        assert!(
            reason.contains(expected),
            "expected {expected:?} in the fault, got: {reason}",
        );
    }
}

#[test]
fn test_reports_broken_for_an_unparseable_config() {
    let system = seed(mock(), "extensions: [ this is not\n");
    let reason = expect_broken(test(&system, &config_path()).unwrap());
    assert!(
        reason.contains("not valid YAML"),
        "reason should name the parse fault: {reason}",
    );
}

#[test]
fn install_preserves_every_byte_outside_the_entry() {
    let original = joined(COMMENTED_CONFIG);
    let system = seed(mock(), &original);
    assert_eq!(install(&system, &config_path()).unwrap(), installed(false));

    let after = system.read_to_string(&config_path()).unwrap();
    assert_eq!(without_entry_block(&after), original);
    assert_eq!(field(&entry_of(&system), "cmd"), Value::from(EXE));
}

/// The sibling declared after the entry and the comments around the block keep their bytes.
#[test]
fn install_repairs_a_drifted_entry_without_reflowing_the_rest() {
    let original = joined(DRIFTED_CONFIG);
    let system = seed(mock(), &original);
    assert_eq!(install(&system, &config_path()).unwrap(), installed(false));

    let after = system.read_to_string(&config_path()).unwrap();
    assert_eq!(without_entry_block(&after), without_entry_block(&original));
    assert_eq!(field(&entry_of(&system), "cmd"), Value::from(EXE));
}

#[test]
fn install_appends_to_a_config_that_declares_no_extensions() {
    let original = joined(&[
        "# goose config -- hand maintained",
        "active_provider: ollama",
    ]);
    let system = seed(mock(), &original);
    assert_eq!(install(&system, &config_path()).unwrap(), installed(false));

    let after = system.read_to_string(&config_path()).unwrap();
    assert!(
        after.starts_with(&original),
        "the user's lines should be untouched above the appended block: {after}",
    );
    assert_eq!(field(&entry_of(&system), "cmd"), Value::from(EXE));
}

#[test]
fn uninstall_removes_only_the_entrys_lines() {
    let original = joined(DRIFTED_CONFIG);
    let system = seed(mock(), &original);
    assert_eq!(
        uninstall(&system, &config_path()).unwrap(),
        uninstalled(false),
    );
    assert_eq!(
        system.read_to_string(&config_path()).unwrap(),
        without_entry_block(&original),
    );
}

/// A bare `extensions:` key would parse as null, so the mapping is emptied in place.
#[test]
fn uninstall_of_the_last_entry_empties_the_mapping_in_place() {
    let system = seed(mock(), &joined(SOLE_ENTRY_CONFIG));
    assert_eq!(
        uninstall(&system, &config_path()).unwrap(),
        uninstalled(false),
    );
    assert_eq!(
        system.read_to_string(&config_path()).unwrap(),
        joined(SOLE_ENTRY_UNINSTALLED),
    );
    assert_eq!(
        config_of(&system).get(Value::from("extensions")),
        Some(&Value::Mapping(Mapping::new())),
    );
}

/// The line editor does not model flow style, so the write re-serializes and the outcome says so.
#[test]
fn install_falls_back_to_reserializing_a_flow_style_extensions_block() {
    let system = seed(
        mock(),
        "active_provider: ollama\nextensions: {developer: {enabled: true}}\n",
    );
    assert_eq!(
        install(&system, &config_path()).unwrap(),
        installed(true),
        "a re-serialized write must report the layout it normalized",
    );

    let config = config_of(&system);
    assert_eq!(
        config.get(Value::from("active_provider")),
        Some(&Value::from("ollama")),
    );
    let extensions = config
        .get(Value::from("extensions"))
        .unwrap()
        .as_mapping()
        .unwrap();
    assert!(
        extensions.get(Value::from("developer")).is_some(),
        "sibling extension dropped: {extensions:?}",
    );
    assert_eq!(field(&entry_of(&system), "cmd"), Value::from(EXE));
}

/// Removal falls back the same way, and reports it the same way.
#[test]
fn uninstall_falls_back_to_reserializing_a_flow_style_extensions_block() {
    let system = seed(
        mock(),
        "active_provider: ollama\nextensions: {remargin: {enabled: true, type: stdio, name: \
         remargin, cmd: /opt/bin/remargin, args: [mcp]}, developer: {enabled: true}}\n",
    );
    assert_eq!(
        uninstall(&system, &config_path()).unwrap(),
        uninstalled(true),
        "a re-serialized write must report the layout it normalized",
    );

    let config = config_of(&system);
    assert_eq!(
        config.get(Value::from("active_provider")),
        Some(&Value::from("ollama")),
    );
    let extensions = config
        .get(Value::from("extensions"))
        .unwrap()
        .as_mapping()
        .unwrap();
    assert!(
        extensions.get(Value::from("developer")).is_some(),
        "sibling extension dropped: {extensions:?}",
    );
    assert!(extensions.get(Value::from(EXTENSION_KEY)).is_none());
}

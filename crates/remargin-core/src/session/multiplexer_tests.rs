//! Tests for the multiplexer engine (task 86). These exercise the *pure*
//! construction — session names, the exact tmux argv vectors, and the full
//! trust-dismiss + seed send-keys sequence — plus the parse/attach surface
//! and the no-supervision invariant. The real-process execution layer is
//! deliberately never spawned here (see the module docs); only the two
//! pre-spawn guard paths of [`launch_into_multiplexer`] are asserted.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::{
    Multiplexer, Tab, build_tmux_plan, launch_into_multiplexer, pane_shows_ready_prompt,
    pane_shows_trust_dialog, session_name,
};

fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).unwrap()
}

fn tab(identity: &str, cwd: &str, launch: &[&str], seeds: &[&str]) -> Tab {
    Tab::new(
        identity.to_owned(),
        PathBuf::from(cwd),
        launch.iter().map(|s| (*s).to_owned()).collect(),
        seeds.iter().map(|s| (*s).to_owned()).collect(),
    )
}

fn strs(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn session_name_is_basename_plus_short_hex() {
    let name = session_name(Path::new("/home/x/demo"), at(1_700_000_000));

    let suffix = name.strip_prefix("demo-").unwrap();
    assert_eq!(suffix.len(), 8, "8 hex chars: {name}");
    assert!(
        suffix.bytes().all(|b| b.is_ascii_hexdigit()),
        "hex suffix: {name}"
    );
}

#[test]
fn session_name_is_deterministic_for_a_fixed_now() {
    let cwd = Path::new("/home/x/demo");
    assert_eq!(session_name(cwd, at(42)), session_name(cwd, at(42)));
}

#[test]
fn session_name_differs_across_two_nows() {
    let cwd = Path::new("/home/x/demo");
    assert_ne!(session_name(cwd, at(42)), session_name(cwd, at(43)));
}

#[test]
fn session_name_sanitizes_unsafe_basename_chars() {
    let name = session_name(Path::new("/vault/TOP OF MIND.base"), at(7));
    assert!(
        name.starts_with("TOP_OF_MIND_base-"),
        "dots and spaces become underscores: {name}"
    );
}

#[test]
fn session_name_falls_back_when_cwd_has_no_basename() {
    let name = session_name(Path::new("/"), at(7));
    assert!(name.starts_with("remargin-"), "root fallback: {name}");
}

#[test]
fn multiplexer_parses_known_values() {
    assert_eq!(Multiplexer::parse("tmux").unwrap(), Multiplexer::Tmux);
    assert_eq!(Multiplexer::parse("zellij").unwrap(), Multiplexer::Zellij);
}

#[test]
fn multiplexer_parse_rejects_unknown_naming_allowed() {
    let err = Multiplexer::parse("screen").unwrap_err().to_string();
    assert!(err.contains("screen"), "names the offender: {err}");
    assert!(err.contains("tmux"), "lists tmux: {err}");
    assert!(err.contains("zellij"), "lists zellij: {err}");
}

#[test]
fn attach_hint_is_multiplexer_specific() {
    assert_eq!(
        Multiplexer::Tmux.attach_hint("demo-abcd"),
        "tmux attach -t demo-abcd"
    );
    assert_eq!(
        Multiplexer::Zellij.attach_hint("demo-abcd"),
        "zellij attach demo-abcd"
    );
}

#[test]
fn tmux_plan_first_tab_is_new_session_rest_are_new_windows() {
    let tabs = [
        tab("root_agent", "/demo", &["claude", "--foo"], &["/loop 30s"]),
        tab(
            "finance",
            "/demo/finance",
            &["claude", "-n", "finance"],
            &["/loop 1h"],
        ),
    ];

    let plan = build_tmux_plan("demo-abcd", &tabs);

    assert_eq!(
        plan.launch[0],
        strs(&[
            "tmux",
            "new-session",
            "-d",
            "-s",
            "demo-abcd",
            "-n",
            "root_agent",
            "-c",
            "/demo",
            "--",
            "claude",
            "--foo",
        ])
    );
    assert_eq!(
        plan.launch[1],
        strs(&[
            "tmux",
            "new-window",
            "-t",
            "demo-abcd",
            "-n",
            "finance",
            "-c",
            "/demo/finance",
            "--",
            "claude",
            "-n",
            "finance",
        ])
    );
    assert_eq!(plan.launch.len(), 2);
}

#[test]
fn tmux_tab_seed_targets_window_by_name() {
    let tabs = [tab(
        "root_agent",
        "/demo",
        &["claude"],
        &["/loop 30s", "/goal go"],
    )];

    let plan = build_tmux_plan("demo-abcd", &tabs);
    let seed = &plan.tabs[0];

    assert_eq!(seed.identity, "root_agent");
    assert_eq!(
        seed.capture,
        strs(&["tmux", "capture-pane", "-t", "demo-abcd:root_agent", "-p"])
    );
    assert_eq!(
        seed.dismiss_trust,
        strs(&["tmux", "send-keys", "-t", "demo-abcd:root_agent", "Enter"])
    );
}

#[test]
fn tmux_seed_lines_type_each_line_then_submit() {
    let tabs = [tab(
        "root_agent",
        "/demo",
        &["claude"],
        &["/loop 30s", "/goal go"],
    )];

    let plan = build_tmux_plan("demo-abcd", &tabs);

    // The full trust-dismiss + seed send-keys sequence, flattened and asserted
    // command-for-command in order.
    let seed = &plan.tabs[0];
    let mut full: Vec<Vec<String>> = vec![seed.dismiss_trust.clone()];
    full.extend(seed.seed_lines.iter().cloned());
    assert_eq!(
        full,
        vec![
            strs(&["tmux", "send-keys", "-t", "demo-abcd:root_agent", "Enter"]),
            strs(&[
                "tmux",
                "send-keys",
                "-t",
                "demo-abcd:root_agent",
                "-l",
                "/loop 30s",
            ]),
            strs(&["tmux", "send-keys", "-t", "demo-abcd:root_agent", "Enter"]),
            strs(&[
                "tmux",
                "send-keys",
                "-t",
                "demo-abcd:root_agent",
                "-l",
                "/goal go",
            ]),
            strs(&["tmux", "send-keys", "-t", "demo-abcd:root_agent", "Enter"]),
        ]
    );
}

#[test]
fn pane_readiness_predicates_match_expected_markers() {
    assert!(pane_shows_trust_dialog(
        "Is this a project you trust? 1. Yes"
    ));
    assert!(!pane_shows_trust_dialog("just a normal prompt"));
    assert!(pane_shows_ready_prompt("> type here  ? for shortcuts"));
    assert!(!pane_shows_ready_prompt(""));
}

#[test]
fn launch_rejects_empty_tabs_without_spawning() {
    let err = launch_into_multiplexer(Multiplexer::Tmux, "demo-abcd", &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains("no sessions to launch"), "message: {err}");
}

#[test]
fn zellij_launch_is_gated_pending_a_bead() {
    // Non-empty tabs so the guard is the zellij gate, not the empty check.
    // This bails before any process is spawned.
    let tabs = [tab("root_agent", "/demo", &["claude"], &["/loop 30s"])];
    let err = launch_into_multiplexer(Multiplexer::Zellij, "demo-abcd", &tabs)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("zellij support pending"),
        "clear pending message: {err}"
    );
    assert!(err.contains("rem-1x1t"), "names the tracking bead: {err}");
}

/// The no-supervision invariant (discussion decisions 3 & 5): the engine
/// must write no PID/registry file. Scan the module source and assert it
/// never references a `.remargin/sessions/` path.
#[test]
fn engine_writes_no_session_registry_path() {
    let src = include_str!("multiplexer.rs");
    assert!(
        !src.contains(".remargin/sessions"),
        "engine must not write a session registry file"
    );
}

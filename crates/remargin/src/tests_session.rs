//! Tests for `remargin session launch --dry-run` (gated on the `session`
//! feature). Exercises `handlers::cmd_session` directly over mock trees.

use std::path::Path;

use os_shim::mock::MockSystem;
use serde_json::Value;

use crate::handlers::cmd_session;
use crate::io::IoSinks;
use crate::{OutputArgs, SessionAction};

fn launch(dry_run: bool, print: bool, identity: Vec<String>, json: bool) -> SessionAction {
    SessionAction::Launch {
        backend: String::from("claude"),
        dry_run,
        identity,
        multiplexer: String::from("tmux"),
        output_args: OutputArgs {
            compact: false,
            json,
            verbose: false,
        },
        print,
    }
}

/// Run `cmd_session` over a mock tree, returning its result and whatever it
/// wrote to stdout. Any error text reaches the user through `dispatch::run`,
/// not the sinks, so the stderr buffer is not inspected here.
fn run(system: &MockSystem, cwd: &str, action: &SessionAction) -> (anyhow::Result<()>, String) {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let result = {
        let mut sinks = IoSinks::new(&mut stdout, &mut stderr);
        cmd_session(&mut sinks, system, Path::new(cwd), action)
    };
    (result, String::from_utf8(stdout).unwrap())
}

/// Root and one child realm, each with a launchable `session:` block.
fn launchable_tree() -> MockSystem {
    MockSystem::new()
        .with_file(
            Path::new("/demo/.remargin.yaml"),
            b"identity: root_agent\nsystem_prompt:\n  name: Root\n  prompt: body\nsession:\n  loop: 30s\n  goal: process pending\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/finance/.remargin.yaml"),
            b"identity: finance\nsession:\n  loop: 30s\n  goal: reconcile\n",
        )
        .unwrap()
}

#[test]
fn dry_run_lists_all_identities() {
    let system = launchable_tree();
    let (result, stdout) = run(&system, "/demo", &launch(true, false, Vec::new(), false));
    result.unwrap();
    assert!(stdout.contains("IDENTITY"), "header missing: {stdout}");
    assert!(stdout.contains("root_agent"), "stdout: {stdout}");
    assert!(stdout.contains("finance"), "stdout: {stdout}");
    assert!(stdout.contains("demo/finance"), "folder path: {stdout}");
    assert!(
        stdout.contains("2 identities; all launchable."),
        "summary: {stdout}"
    );
}

#[test]
fn dry_run_flags_missing_goal_and_exits_nonzero() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/demo/.remargin.yaml"),
            b"identity: root_agent\nsession:\n  loop: 30s\n  goal: go\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/ops/.remargin.yaml"),
            b"identity: ops\nsession:\n  loop: 30s\n",
        )
        .unwrap();
    let (result, stdout) = run(&system, "/demo", &launch(true, false, Vec::new(), false));
    assert!(result.is_err(), "a missing goal must exit non-zero");
    assert!(stdout.contains("MISSING goal"), "flag missing: {stdout}");
    assert!(stdout.contains("1 not launchable"), "summary: {stdout}");
}

#[test]
fn dry_run_json_is_structured_array() {
    let system = launchable_tree();
    let (result, stdout) = run(&system, "/demo", &launch(true, false, Vec::new(), true));
    result.unwrap();
    let parsed: Value = serde_json::from_str(&stdout).unwrap();
    let array = parsed.as_array().unwrap();
    assert_eq!(array.len(), 2);
    for entry in array {
        assert_eq!(entry["launchable"], Value::Bool(true));
        assert!(entry.get("identity").is_some(), "identity key: {entry}");
        assert!(entry.get("loop").is_some(), "loop key: {entry}");
        assert!(entry.get("goal").is_some(), "goal key: {entry}");
    }
}

#[test]
fn dry_run_identity_filter_restricts_rows() {
    let system = launchable_tree();
    let (result, stdout) = run(
        &system,
        "/demo",
        &launch(true, false, vec![String::from("finance")], false),
    );
    result.unwrap();
    assert!(stdout.contains("finance"), "stdout: {stdout}");
    assert!(
        !stdout.contains("root_agent"),
        "filter should drop root_agent: {stdout}"
    );
    assert!(
        stdout.contains("1 identities; all launchable."),
        "summary: {stdout}"
    );
}

/// A bare launch that names an unknown multiplexer must fail on the flag
/// before touching any session — and, crucially, without spawning tmux
/// (which the gate must never do).
#[test]
fn bare_launch_rejects_unknown_multiplexer() {
    let system = launchable_tree();
    let action = SessionAction::Launch {
        backend: String::from("claude"),
        dry_run: false,
        identity: Vec::new(),
        multiplexer: String::from("screen"),
        output_args: OutputArgs {
            compact: false,
            json: false,
            verbose: false,
        },
        print: false,
    };
    let (result, stdout) = run(&system, "/demo", &action);
    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("screen"),
        "names the offender: {err:#}"
    );
    assert!(
        format!("{err:#}").contains("tmux") && format!("{err:#}").contains("zellij"),
        "lists allowed values: {err:#}"
    );
    assert!(stdout.is_empty(), "no output on a flag error: {stdout}");
}

/// zellij is parsed but its launch path is gated pending a follow-up; a bare
/// `--multiplexer zellij` launch surfaces that clearly, spawning nothing.
#[test]
fn bare_launch_zellij_is_gated_pending() {
    let system = launchable_tree();
    let action = SessionAction::Launch {
        backend: String::from("claude"),
        dry_run: false,
        identity: Vec::new(),
        multiplexer: String::from("zellij"),
        output_args: OutputArgs {
            compact: false,
            json: false,
            verbose: false,
        },
        print: false,
    };
    let (result, _stdout) = run(&system, "/demo", &action);
    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("zellij support pending"),
        "clear pending message: {err:#}"
    );
}

/// A bare (real) launch builds every identity's spec first, so a missing
/// `goal` surfaces the task-84 error before any multiplexer command is
/// spawned. This keeps the launch branch under test without a real tmux.
#[test]
fn bare_launch_surfaces_task84_error_before_spawning() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/demo/.remargin.yaml"),
            b"identity: root_agent\nsession:\n  loop: 30s\n",
        )
        .unwrap();
    let (result, stdout) = run(&system, "/demo", &launch(false, false, Vec::new(), false));
    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("`goal` is required"),
        "task-84 error surfaces: {err:#}"
    );
    assert!(
        stdout.is_empty(),
        "nothing printed before the error: {stdout}"
    );
}

/// A bare launch with no discovered identity bails clearly rather than
/// spawning an empty session.
#[test]
fn bare_launch_no_identities_bails() {
    let system = MockSystem::new()
        .with_file(Path::new("/demo/.remargin.yaml"), b"mode: open\n")
        .unwrap();
    let (result, _stdout) = run(&system, "/demo", &launch(false, false, Vec::new(), false));
    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("no launchable identities"),
        "clear empty message: {err:#}"
    );
}

#[test]
fn print_emits_launch_command_and_seed_lines() {
    let system = launchable_tree();
    let (result, stdout) = run(&system, "/demo", &launch(false, true, Vec::new(), false));
    result.unwrap();

    assert!(stdout.contains("# root_agent"), "header: {stdout}");
    assert!(stdout.contains("# finance"), "header: {stdout}");
    // Runnable, interactive launch line -- never headless `claude -p`.
    assert!(stdout.contains("cd /demo &&"), "cd line: {stdout}");
    assert!(
        stdout.contains("claude --append-system-prompt"),
        "argv: {stdout}"
    );
    assert!(stdout.contains("--strict-mcp-config"), "argv: {stdout}");
    assert!(stdout.contains("--permission-mode auto"), "argv: {stdout}");
    assert!(!stdout.contains(" -p "), "must not be headless: {stdout}");
    // Seed lines are typed into the session, not passed as flags.
    assert!(stdout.contains("/loop 30s"), "loop seed: {stdout}");
    assert!(
        stdout.contains("/goal process pending"),
        "goal seed: {stdout}"
    );
    assert!(stdout.contains("/goal reconcile"), "goal seed: {stdout}");
}

#[test]
fn print_surfaces_task84_error_for_missing_goal() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/demo/.remargin.yaml"),
            b"identity: root_agent\nsession:\n  loop: 30s\n",
        )
        .unwrap();
    let (result, _stdout) = run(&system, "/demo", &launch(false, true, Vec::new(), false));
    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("`goal` is required"),
        "task-84 error surfaces: {err:#}"
    );
}

#[test]
fn print_unknown_backend_lists_known() {
    let system = launchable_tree();
    let action = SessionAction::Launch {
        backend: String::from("bogus"),
        dry_run: false,
        identity: Vec::new(),
        multiplexer: String::from("tmux"),
        output_args: OutputArgs {
            compact: false,
            json: false,
            verbose: false,
        },
        print: true,
    };
    let (result, _stdout) = run(&system, "/demo", &action);
    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("bogus"),
        "names offender: {err:#}"
    );
    assert!(
        format!("{err:#}").contains("claude"),
        "lists known: {err:#}"
    );
}

/// The launch path must write no PID/registry file (discussion decisions 3
/// & 5). Scan the handler source and assert it never references a
/// `.remargin/sessions/` path.
#[test]
fn launch_handler_writes_no_session_registry_path() {
    use std::fs;

    let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/handlers.rs")).unwrap();
    assert!(
        !src.contains(".remargin/sessions"),
        "the launch handler must not write a session registry file"
    );
}

/// The `--print` path renders commands; it must never spawn a child
/// process. Rather than fake an intercept, scan the source of every
/// function on that path and assert none references `Command` (the only
/// route to `spawn`/`status`/`output`).
#[test]
fn print_path_spawns_no_child_process() {
    use std::fs;

    use syn::spanned::Spanned as _;
    use syn::{Item, ItemFn};

    const PRINT_PATH_FNS: &[&str] = &[
        "cmd_session",
        "render_session_print",
        "shell_join",
        "shell_quote",
    ];

    let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/handlers.rs")).unwrap();
    let lines: Vec<&str> = src.lines().collect();
    let file = syn::parse_file(&src).unwrap();

    let mut scanned: Vec<String> = Vec::new();
    for item in &file.items {
        if let Item::Fn(ItemFn { sig, block, .. }) = item {
            let name = sig.ident.to_string();
            if !PRINT_PATH_FNS.contains(&name.as_str()) {
                continue;
            }
            let start = sig.ident.span().start().line;
            let end = block.span().end().line;
            let body = lines[start - 1..end].join("\n");
            assert!(
                !body.contains("Command"),
                "`{name}` on the --print path references `Command`: it must spawn nothing"
            );
            scanned.push(name);
        }
    }

    for expected in PRINT_PATH_FNS {
        assert!(
            scanned.iter().any(|name| name == expected),
            "expected to scan `{expected}` in handlers.rs but it was not found"
        );
    }
}

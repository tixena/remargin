//! Tests for `remargin session launch --dry-run` (gated on the `session`
//! feature). Exercises `handlers::cmd_session` directly over mock trees.

use std::path::Path;

use clap::Parser as _;
use clap::error::ErrorKind;
use os_shim::mock::MockSystem;
use serde_json::Value;

use crate::handlers::cmd_session;
use crate::io::IoSinks;
use crate::{Cli, OutputArgs, SessionAction};

/// The `./product` entry's own config: a launchable `session:` block the
/// manifest entry overrides (`entry goal` / `2m` win over these).
const CANONICAL_PRODUCT: &[u8] = b"identity: product\nsession:\n  goal: own goal\n  loop: 30s\n";

/// The `/lib/researcher` entry's own config: no manifest override rides on it,
/// so its `session:` block is what launches.
const CANONICAL_RESEARCHER: &[u8] = b"identity: researcher\nsession:\n  goal: research goal\n";

/// `/ws` config for the canonical manifest tree: its own `ws_root` identity
/// and a `session:` block, plus a `sessions:` manifest whose `evaluation`
/// session rosters a `./product` entry (goal+loop overrides) and an
/// absolute-path `/lib/researcher` entry, and whose `broken` session points at
/// a missing folder. `default: evaluation` makes a bare launch resolve it.
const WS_MANIFEST: &[u8] = b"identity: ws_root\nsession:\n  goal: root goal\nsessions:\n  default: evaluation\n  evaluation:\n    agents:\n      - path: ./product\n        goal: entry goal\n        loop: 2m\n      - path: /lib/researcher\n  broken:\n    agents:\n      - path: ./missing\n";

/// Build a `session launch` action over the mock `tmux` multiplexer. `name`
/// selects a manifest session when `Some`; `None` is a bare launch. All modes
/// (dry-run, print, real launch) are reachable through the knobs.
fn session_launch(
    name: Option<&str>,
    dry_run: bool,
    print: bool,
    identity: Vec<String>,
    json: bool,
) -> SessionAction {
    SessionAction::Launch {
        dry_run,
        identity,
        multiplexer: Some(String::from("tmux")),
        name: name.map(String::from),
        output_args: OutputArgs {
            compact: false,
            json,
            verbose: false,
        },
        print,
    }
}

fn launch(dry_run: bool, print: bool, identity: Vec<String>, json: bool) -> SessionAction {
    session_launch(None, dry_run, print, identity, json)
}

/// Like [`launch`], but names a manifest session. Always a `--dry-run` so the
/// resolved fleet is rendered without spawning a multiplexer.
fn launch_named(name: &str, identity: Vec<String>) -> SessionAction {
    session_launch(Some(name), true, false, identity, false)
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

/// A `sessions:` manifest workspace. `/ws` declares two named sessions over
/// out-of-tree agent folders and no identity of its own, so downward
/// discovery from `/ws` is empty and the launched fleet is exactly the named
/// roster the manifest resolves.
fn manifest_workspace() -> MockSystem {
    MockSystem::new()
        .with_file(
            Path::new("/ws/.remargin.yaml"),
            b"sessions:\n  default: evaluation\n  \
              evaluation:\n    agents:\n      \
              - path: /agents/product\n      - path: /agents/researcher\n  \
              solo:\n    agents:\n      - path: /agents/product\n",
        )
        .unwrap()
        .with_file(
            Path::new("/agents/product/.remargin.yaml"),
            b"identity: product\nsession:\n  loop: 2m\n  goal: evaluate the brief\n",
        )
        .unwrap()
        .with_file(
            Path::new("/agents/researcher/.remargin.yaml"),
            b"identity: researcher\nsession:\n  goal: research prior art\n",
        )
        .unwrap()
}

/// The canonical manifest tree, with each roster member's own config
/// injectable so a scenario can perturb exactly one of them (drop a `goal`,
/// add an unknown key) while the rest of the tree stays fixed.
fn canonical_workspace_with(product: &[u8], researcher: &[u8]) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/ws/.remargin.yaml"), WS_MANIFEST)
        .unwrap()
        .with_file(Path::new("/ws/product/.remargin.yaml"), product)
        .unwrap()
        .with_file(Path::new("/lib/researcher/.remargin.yaml"), researcher)
        .unwrap()
}

/// The unperturbed canonical tree used by the happy-path dry-run/print rows.
fn canonical_workspace() -> MockSystem {
    canonical_workspace_with(CANONICAL_PRODUCT, CANONICAL_RESEARCHER)
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
    let err = result.unwrap_err();
    assert!(stdout.contains("MISSING goal"), "flag missing: {stdout}");
    assert!(stdout.contains("1 not launchable"), "summary: {stdout}");
    assert!(stdout.contains("missing goal"), "summary wording: {stdout}");
    assert!(
        !stdout.contains("loop/goal"),
        "no stale loop/goal wording: {stdout}"
    );
    assert!(
        format!("{err:#}").contains("missing goal"),
        "bail wording names goal only: {err:#}"
    );
}

/// A goal-only session (no `loop`) is launchable: the builder defaults the
/// cadence to `5m`, and the dry-run loop cell says so rather than flagging a
/// missing value.
#[test]
fn dry_run_defaulted_loop_renders_5m_default() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/demo/.remargin.yaml"),
            b"identity: finance\nsession:\n  goal: process pending work\n",
        )
        .unwrap();
    let (result, stdout) = run(&system, "/demo", &launch(true, false, Vec::new(), false));
    result.unwrap();
    assert!(
        stdout.contains("5m (default)"),
        "loop cell defaults: {stdout}"
    );
    assert!(
        !stdout.contains("MISSING loop"),
        "absent loop is not a blocker: {stdout}"
    );
    assert!(
        stdout.contains("1 identities; all launchable."),
        "goal-only session is launchable: {stdout}"
    );
}

/// The `--json` output agrees with the table: a defaulted loop reports
/// `5m (default)` and the entry is launchable.
#[test]
fn dry_run_json_defaulted_loop_reports_5m_default() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/demo/.remargin.yaml"),
            b"identity: finance\nsession:\n  goal: process pending work\n",
        )
        .unwrap();
    let (result, stdout) = run(&system, "/demo", &launch(true, false, Vec::new(), true));
    result.unwrap();
    let parsed: Value = serde_json::from_str(&stdout).unwrap();
    let entry = &parsed.as_array().unwrap()[0];
    assert_eq!(entry["loop"], Value::String(String::from("5m (default)")));
    assert_eq!(entry["launchable"], Value::Bool(true));
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
        dry_run: false,
        identity: Vec::new(),
        multiplexer: Some(String::from("screen")),
        name: None,
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
        format!("{err:#}").contains("herdr") && format!("{err:#}").contains("tmux"),
        "lists allowed values: {err:#}"
    );
    assert!(stdout.is_empty(), "no output on a flag error: {stdout}");
}

/// zellij was removed in favour of herdr; naming it now fails on the flag,
/// listing the allowed values (herdr, tmux), and spawns nothing.
#[test]
fn bare_launch_rejects_zellij_now_removed() {
    let system = launchable_tree();
    let action = SessionAction::Launch {
        dry_run: false,
        identity: Vec::new(),
        multiplexer: Some(String::from("zellij")),
        name: None,
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
        format!("{err:#}").contains("zellij"),
        "names the offender: {err:#}"
    );
    assert!(
        format!("{err:#}").contains("herdr") && format!("{err:#}").contains("tmux"),
        "lists allowed values: {err:#}"
    );
    assert!(stdout.is_empty(), "no output on a flag error: {stdout}");
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

/// `--backend` was retired: the backend is inferred per agent, so the flag no
/// longer exists and clap rejects it as an unexpected argument. (Unknown
/// backend *names* are still covered by `resolve_backend`'s own unit test.)
#[test]
fn launch_rejects_retired_backend_flag() {
    let err = Cli::try_parse_from(["remargin", "session", "launch", "--backend", "claude"])
        .err()
        .unwrap();
    assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    assert!(
        err.to_string().contains("--backend"),
        "names the retired flag: {err}"
    );
}

/// Launch by name resolves the manifest's named fleet: `evaluation` rosters
/// both agents, and the dry-run lists exactly those two identities.
#[test]
fn dry_run_by_name_resolves_manifest_fleet() {
    let system = manifest_workspace();
    let (result, stdout) = run(&system, "/ws", &launch_named("evaluation", Vec::new()));
    result.unwrap();
    assert!(stdout.contains("product"), "stdout: {stdout}");
    assert!(stdout.contains("researcher"), "stdout: {stdout}");
    assert!(
        stdout.contains("2 identities; all launchable."),
        "summary: {stdout}"
    );
}

/// A bare launch over a manifest applies the settled default rule: the
/// declared `default: evaluation` session is resolved, not empty discovery.
#[test]
fn dry_run_bare_over_manifest_uses_default_rule() {
    let system = manifest_workspace();
    let (result, stdout) = run(&system, "/ws", &launch(true, false, Vec::new(), false));
    result.unwrap();
    assert!(stdout.contains("product"), "stdout: {stdout}");
    assert!(stdout.contains("researcher"), "stdout: {stdout}");
    assert!(
        stdout.contains("2 identities; all launchable."),
        "default rule resolves the evaluation fleet: {stdout}"
    );
}

/// The `--identity` filter applies to the resolved union fleet, not just
/// downward discovery: naming `researcher` drops `product` from the roster.
#[test]
fn dry_run_identity_filter_applies_to_named_fleet() {
    let system = manifest_workspace();
    let (result, stdout) = run(
        &system,
        "/ws",
        &launch_named("evaluation", vec![String::from("researcher")]),
    );
    result.unwrap();
    assert!(stdout.contains("researcher"), "stdout: {stdout}");
    assert!(
        !stdout.contains("product"),
        "filter should drop product: {stdout}"
    );
    assert!(
        stdout.contains("1 identities; all launchable."),
        "summary: {stdout}"
    );
}

// -- Canonical manifest tree: dry-run/print faithfulness + all-or-nothing --

/// Dry-run over the canonical `evaluation` fleet renders the union in
/// entry-first order (`product`, `researcher`, then the discovered `ws_root`),
/// with the `./product` entry's overrides winning over its own config and the
/// no-override members showing `5m (default)` cadence.
#[test]
fn canonical_dry_run_union_table_orders_entries_first_with_overrides() {
    let system = canonical_workspace();
    let (result, stdout) = run(&system, "/ws", &launch_named("evaluation", Vec::new()));
    result.unwrap();

    assert!(
        stdout.contains("3 identities; all launchable."),
        "three union members: {stdout}"
    );
    let product = stdout.find("product").unwrap();
    let researcher = stdout.find("researcher").unwrap();
    let ws_root = stdout.find("ws_root").unwrap();
    assert!(
        product < ws_root && researcher < ws_root,
        "manifest entries precede the discovered root: {stdout}"
    );
    // The entry's goal+loop overrides win over product's own `session:` block.
    assert!(
        stdout.contains("entry goal"),
        "override goal shown: {stdout}"
    );
    assert!(
        !stdout.contains("own goal"),
        "entry override replaces the folder's own goal: {stdout}"
    );
    assert!(stdout.contains("2m"), "override loop shown: {stdout}");
    // No-override members keep their own goal and default the cadence.
    assert!(
        stdout.contains("research goal"),
        "researcher goal: {stdout}"
    );
    assert!(stdout.contains("root goal"), "root goal: {stdout}");
    assert!(
        stdout.contains("5m (default)"),
        "defaulted cadence cells: {stdout}"
    );
}

/// The `--json` array agrees with the table: same entry-first order, same
/// applied overrides, and the same `5m (default)` marker on defaulted members.
#[test]
fn canonical_dry_run_union_json_agrees_with_table() {
    let system = canonical_workspace();
    let (result, stdout) = run(
        &system,
        "/ws",
        &session_launch(Some("evaluation"), true, false, Vec::new(), true),
    );
    result.unwrap();

    let parsed: Value = serde_json::from_str(&stdout).unwrap();
    let array = parsed.as_array().unwrap();
    assert_eq!(array.len(), 3, "three union members: {stdout}");
    assert_eq!(array[0]["identity"], Value::String(String::from("product")));
    assert_eq!(
        array[1]["identity"],
        Value::String(String::from("researcher"))
    );
    assert_eq!(array[2]["identity"], Value::String(String::from("ws_root")));
    assert_eq!(array[0]["loop"], Value::String(String::from("2m")));
    assert_eq!(array[0]["goal"], Value::String(String::from("entry goal")));
    assert_eq!(
        array[1]["loop"],
        Value::String(String::from("5m (default)"))
    );
    assert_eq!(
        array[2]["loop"],
        Value::String(String::from("5m (default)"))
    );
    for entry in array {
        assert_eq!(entry["launchable"], Value::Bool(true));
    }
}

/// Dry-run over the `broken` session fails fleet resolution, naming the
/// offending session and its missing entry, and prints no partial table.
#[test]
fn canonical_dry_run_bad_entry_errs_naming_missing_with_no_table() {
    let system = canonical_workspace();
    let (result, stdout) = run(&system, "/ws", &launch_named("broken", Vec::new()));
    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("broken"),
        "names the offending session: {err:#}"
    );
    assert!(
        format!("{err:#}").contains("./missing"),
        "names the offending entry: {err:#}"
    );
    assert!(
        stdout.is_empty(),
        "no partial table before the resolution error: {stdout}"
    );
}

/// Print over the canonical fleet emits a `cd … && claude …` line for every
/// union member plus each member's `/loop` + `/goal` seeds, with the product
/// entry's overrides reflected in its seeds.
#[test]
fn canonical_print_emits_cd_and_seeds_for_every_member() {
    let system = canonical_workspace();
    let (result, stdout) = run(
        &system,
        "/ws",
        &session_launch(Some("evaluation"), false, true, Vec::new(), false),
    );
    result.unwrap();

    assert!(stdout.contains("# product"), "product header: {stdout}");
    assert!(
        stdout.contains("# researcher"),
        "researcher header: {stdout}"
    );
    assert!(stdout.contains("# ws_root"), "ws_root header: {stdout}");
    assert!(
        stdout.contains("cd /ws/product &&"),
        "product cd line: {stdout}"
    );
    assert!(
        stdout.contains("cd /lib/researcher &&"),
        "researcher cd line: {stdout}"
    );
    assert!(stdout.contains("cd /ws &&"), "ws_root cd line: {stdout}");
    assert!(
        stdout.contains("claude --append-system-prompt"),
        "renders the interactive launch argv: {stdout}"
    );
    // Seeds carry the overridden cadence/goal for product and the defaults for
    // the no-override members.
    assert!(
        stdout.contains("/loop 2m"),
        "product override loop: {stdout}"
    );
    assert!(
        stdout.contains("/goal entry goal"),
        "product override goal: {stdout}"
    );
    assert!(
        stdout.contains("/loop 5m"),
        "defaulted cadence seed for no-override members: {stdout}"
    );
    assert!(
        stdout.contains("/goal research goal"),
        "researcher goal seed: {stdout}"
    );
    assert!(
        stdout.contains("/goal root goal"),
        "root goal seed: {stdout}"
    );
}

/// All-or-nothing: a bad manifest entry aborts a real launch during fleet
/// resolution — no `Launched` line, nothing printed, no multiplexer reached.
#[test]
fn canonical_launch_aborts_on_bad_entry_before_any_output() {
    let system = canonical_workspace();
    let (result, stdout) = run(
        &system,
        "/ws",
        &session_launch(Some("broken"), false, false, Vec::new(), false),
    );
    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("./missing"),
        "names the offending entry: {err:#}"
    );
    assert!(
        !stdout.contains("Launched"),
        "no Launched line on abort: {stdout}"
    );
    assert!(
        stdout.is_empty(),
        "nothing printed before the abort: {stdout}"
    );
}

/// All-or-nothing: a member missing `goal` (here `/lib/researcher`, which
/// carries no override goal) aborts the launch while building specs — before
/// the multiplexer and before any output.
#[test]
fn canonical_launch_aborts_on_member_missing_goal() {
    let system = canonical_workspace_with(
        CANONICAL_PRODUCT,
        b"identity: researcher\nsession:\n  loop: 1m\n",
    );
    let (result, stdout) = run(
        &system,
        "/ws",
        &session_launch(Some("evaluation"), false, false, Vec::new(), false),
    );
    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("researcher"),
        "names the offending member: {err:#}"
    );
    assert!(
        format!("{err:#}").contains("goal"),
        "names the missing field: {err:#}"
    );
    assert!(
        !stdout.contains("Launched"),
        "no Launched line on abort: {stdout}"
    );
    assert!(
        stdout.is_empty(),
        "nothing printed before the member error: {stdout}"
    );
}

/// All-or-nothing: a strict-parse error in a member's own config (a typo'd
/// `gaol:` key rejected by `deny_unknown_fields`) aborts fleet resolution
/// before any spec is built — no `Launched` line, nothing printed.
#[test]
fn canonical_launch_aborts_on_strict_parse_error() {
    let system = canonical_workspace_with(
        b"identity: product\nsession:\n  goal: own goal\n  loop: 30s\n  gaol: x\n",
        CANONICAL_RESEARCHER,
    );
    let (result, stdout) = run(
        &system,
        "/ws",
        &session_launch(Some("evaluation"), false, false, Vec::new(), false),
    );
    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("gaol"),
        "surfaces the unknown key: {err:#}"
    );
    assert!(
        !stdout.contains("Launched"),
        "no Launched line on abort: {stdout}"
    );
    assert!(
        stdout.is_empty(),
        "nothing printed before the parse error: {stdout}"
    );
}

/// The `--identity` filter narrows the resolved union to a single manifest
/// entry, and that entry's override still applies under the filter.
#[test]
fn canonical_identity_filter_selects_single_entry_row() {
    let system = canonical_workspace();
    let (result, stdout) = run(
        &system,
        "/ws",
        &launch_named("evaluation", vec![String::from("product")]),
    );
    result.unwrap();
    assert!(
        stdout.contains("product"),
        "keeps the named member: {stdout}"
    );
    assert!(!stdout.contains("researcher"), "drops researcher: {stdout}");
    assert!(!stdout.contains("ws_root"), "drops ws_root: {stdout}");
    assert!(
        stdout.contains("1 identities; all launchable."),
        "summary: {stdout}"
    );
    assert!(
        stdout.contains("entry goal"),
        "the entry override survives the filter: {stdout}"
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

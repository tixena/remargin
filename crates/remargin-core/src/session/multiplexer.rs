//! Multiplexer orchestration for `remargin session launch`.
//!
//! Where [`super::backend`] renders one identity's interactive launch argv
//! and its `/loop` + `/goal` seed lines, this module puts those into a new
//! **named** terminal-multiplexer session: one identity per tab, each
//! running its launch command, seeded by typing the slash-commands into the
//! live TUI via the multiplexer's send-keys. remargin only *starts* the
//! session — it writes no PID/registry file and never stops, reaps, or
//! supervises what it launched (discussion decisions 3 & 5).
//!
//! The module splits cleanly into two halves so the interesting part is
//! deterministic and unit-testable without spawning anything:
//!
//! - **Pure construction** ([`session_name`], [`build_tmux_plan`],
//!   [`build_herdr_plan`]) turns a `(session_name, tabs)` pair into the exact
//!   multiplexer command argv vectors and the full trust-dismiss + seed
//!   sequence. These are asserted command-for-command in the tests.
//! - **Thin execution** ([`launch_into_multiplexer`]) spawns the constructed
//!   commands via [`std::process::Command`]. The tmux path polls `capture-pane`
//!   for readiness; the herdr path uses herdr's blocking `wait` primitives
//!   instead. It is deliberately kept out of the quality gate: it needs a real
//!   tmux/herdr/`claude` and would be flaky, so nothing here calls it.
//!
//! herdr is the flagship: an agent-aware workspace manager that addresses
//! tabs/panes/agents **by name**, exposes blocking `wait` primitives, and
//! natively detects Claude's state. It is the default when its server is
//! reachable; tmux is the zero-extra-dependency fallback and stays the choice
//! on hosts without herdr.

use core::fmt::Write as _;
use core::time::Duration;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

/// Placeholder tokens the herdr execution layer substitutes with the real ids
/// parsed from herdr's JSON: `workspace_id` from `workspace create`, `tab_id`
/// from `workspace create`'s default tab (identity 0) or each `tab create`
/// (identities 1..N), and `pane_id` from each `agent start`.
const HERDR_PANE_PLACEHOLDER: &str = "<PANE>";
const HERDR_TAB_PLACEHOLDER: &str = "<TAB>";
const HERDR_WORKSPACE_PLACEHOLDER: &str = "<WS>";

/// `herdr wait agent-status --timeout` for the idle prompt, in milliseconds.
const HERDR_IDLE_TIMEOUT_MS: u32 = 35_000;
/// `herdr wait output --timeout` for the trust-dialog probe, in milliseconds.
/// Best-effort: an already-trusted folder shows no dialog and this simply
/// expires, so keep it short — the real readiness gate is `wait agent-status`.
const HERDR_TRUST_TIMEOUT_MS: u32 = 10_000;

/// Bounded readiness poll: at most this many `capture-pane` reads before
/// seeding proceeds anyway. With [`READINESS_POLL`] this caps the wait near
/// ten seconds per tab.
const READINESS_MAX_POLLS: u32 = 40;
/// Delay between readiness polls and after dismissing the trust dialog.
const READINESS_POLL: Duration = Duration::from_millis(250);

/// A terminal multiplexer `remargin session launch` can target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Multiplexer {
    /// herdr — the flagship: agent-aware, name-addressed, blocking waits.
    Herdr,
    /// tmux — the zero-extra-dependency fallback.
    Tmux,
}

impl Multiplexer {
    /// A human-facing hint for reattaching to `session_name`.
    ///
    /// The herdr launch creates a **workspace** labeled `session_name` inside
    /// herdr's **default** session — it is not itself a herdr session, so the
    /// hint attaches to `default` and names the workspace to open, not
    /// `herdr session attach <session_name>` (which would not resolve).
    #[must_use]
    pub fn attach_hint(self, session_name: &str) -> String {
        match self {
            Self::Herdr => {
                format!("herdr session attach default   # then open workspace {session_name}")
            }
            Self::Tmux => format!("tmux attach -t {session_name}"),
        }
    }

    /// The kind of container the launch creates, for user-facing messages: a
    /// herdr **workspace** (inside herdr's default session) or a tmux
    /// **session**. herdr's is a workspace, not a session — calling it a
    /// session would send users to a `herdr session attach` that fails.
    #[must_use]
    pub const fn container_kind(self) -> &'static str {
        match self {
            Self::Herdr => "workspace",
            Self::Tmux => "session",
        }
    }

    /// Stable lowercase name (`"herdr"` / `"tmux"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Herdr => "herdr",
            Self::Tmux => "tmux",
        }
    }

    /// Parse the `--multiplexer` value.
    ///
    /// # Errors
    ///
    /// Returns an error naming the allowed values when `value` is neither
    /// `herdr` nor `tmux`.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "herdr" => Ok(Self::Herdr),
            "tmux" => Ok(Self::Tmux),
            other => bail!("unknown multiplexer {other:?}; allowed: herdr, tmux"),
        }
    }
}

/// One identity's tab: where to launch, what to launch, and what to type in
/// once it is live.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Tab {
    /// Working directory the tab launches in.
    pub cwd: PathBuf,
    /// Identity governing the tab; also the tab/window name.
    pub identity: String,
    /// Interactive launch argv (`claude …`) run as the tab's command.
    pub launch_argv: Vec<String>,
    /// Slash-command lines typed into the live session (`/loop …`, `/goal …`).
    pub seed_inputs: Vec<String>,
}

impl Tab {
    /// Assemble a tab from its identity, working directory, launch argv, and
    /// seed lines.
    #[must_use]
    pub const fn new(
        identity: String,
        cwd: PathBuf,
        launch_argv: Vec<String>,
        seed_inputs: Vec<String>,
    ) -> Self {
        Self {
            cwd,
            identity,
            launch_argv,
            seed_inputs,
        }
    }
}

/// The seed choreography for one tmux tab: how to read its pane for
/// readiness, how to dismiss the workspace-trust dialog, and the ordered
/// send-keys pairs that type each seed line and submit it.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct TmuxTabSeed {
    /// `capture-pane` argv polled for the trust dialog and ready prompt.
    pub capture: Vec<String>,
    /// `send-keys … Enter` that accepts the workspace-trust dialog.
    pub dismiss_trust: Vec<String>,
    /// Identity of the tab this seeds (its window name).
    pub identity: String,
    /// Ordered send-keys commands: for each seed line, a literal `-l <line>`
    /// followed by an `Enter` submit.
    pub seed_lines: Vec<Vec<String>>,
}

/// A fully-constructed tmux launch plan, deterministic given
/// `(session_name, tabs)`.
///
/// [`Self::launch`] is the create + per-tab `new-window` commands;
/// [`Self::tabs`] is the per-tab seed choreography. The execution layer walks
/// both, interleaving readiness polling that is not represented here.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct TmuxPlan {
    /// `new-session` (first tab) then one `new-window` per further tab.
    pub launch: Vec<Vec<String>>,
    /// Per-tab seed choreography, in tab order.
    pub tabs: Vec<TmuxTabSeed>,
}

/// A fully-constructed herdr launch plan, deterministic given
/// `(session_name, tabs)`.
///
/// Pure: spawns nothing. `<WS>`/`<TAB>`/`<PANE>` placeholder tokens are
/// substituted by the thin execution layer from the ids it parses out of
/// herdr's JSON responses at run time.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct HerdrPlan {
    /// `herdr workspace create …` for the named session.
    pub create_workspace: Vec<String>,
    /// Per tab, in order: optional `tab create`, `agent start`, and wait/seed
    /// choreography. One tab per identity — identity 0 reuses the workspace's
    /// default tab; identities 1..N create their own.
    pub tabs: Vec<HerdrTabPlan>,
}

/// One identity's herdr choreography within a plan.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct HerdrTabPlan {
    /// `herdr agent start <identity> --tab <TAB> --cwd <cwd> --no-focus
    /// -- <launch_argv>`. `<TAB>` is resolved from the workspace's default tab
    /// (identity 0) or this tab's [`Self::tab_create`] JSON (identities 1..N).
    pub agent_start: Vec<String>,
    /// `herdr pane send-keys <PANE> enter` — accepts the workspace-trust
    /// dialog. Sent only when [`Self::wait_trust`] actually matched; on an
    /// already-trusted folder the dialog never appears and this is skipped.
    pub dismiss_trust_enter: Vec<String>,
    /// Identity governing the tab; also the herdr agent name.
    pub identity: String,
    /// Ordered seed commands: for each seed line, `herdr agent send <identity>
    /// <line>` then `herdr pane send-keys <PANE> enter`.
    pub seed: Vec<Vec<String>>,
    /// `herdr tab create --workspace <WS> --cwd <cwd> --label <identity>
    /// --no-focus` for identities 1..N. Empty for identity 0, which reuses the
    /// workspace's default tab so no empty leftover pane is created.
    pub tab_create: Vec<String>,
    /// Required readiness gate: `herdr wait agent-status <PANE> --status idle`.
    pub wait_idle: Vec<String>,
    /// Best-effort trust probe: `herdr wait output <PANE> --match trust`. A
    /// timeout here means the folder was already trusted (no dialog appeared) —
    /// it is NOT fatal; the launch proceeds to [`Self::wait_idle`] regardless.
    pub wait_trust: Vec<String>,
}

/// Minimal typed view of a `herdr agent start` response: only the agent's
/// `pane_id`.
#[derive(Debug, Deserialize)]
struct HerdrAgent {
    pane_id: String,
}

/// One pane in a `herdr pane list` response: its id and the tab it belongs to.
#[derive(Debug, Deserialize)]
struct HerdrPaneEntry {
    pane_id: String,
    tab_id: String,
}

/// Minimal typed view of a `herdr tab create` response's `tab`: its `tab_id`.
#[derive(Debug, Deserialize)]
struct HerdrTab {
    tab_id: String,
}

/// Minimal typed view of a `herdr workspace create` response's `workspace`:
/// its `workspace_id`.
#[derive(Debug, Deserialize)]
struct HerdrWorkspace {
    workspace_id: String,
}

#[derive(Debug, Deserialize)]
struct HerdrAgentResult {
    agent: HerdrAgent,
}

#[derive(Debug, Deserialize)]
struct HerdrAgentStarted {
    result: HerdrAgentResult,
}

#[derive(Debug, Deserialize)]
struct HerdrPaneList {
    result: HerdrPaneListResult,
}

#[derive(Debug, Deserialize)]
struct HerdrPaneListResult {
    panes: Vec<HerdrPaneEntry>,
}

/// The default tab herdr opens with a new workspace, carried in `workspace
/// create`'s `root_pane`. Reused for identity 0 so no empty pane is left over.
#[derive(Debug, Deserialize)]
struct HerdrRootPane {
    tab_id: String,
}

#[derive(Debug, Deserialize)]
struct HerdrTabCreated {
    result: HerdrTabResult,
}

#[derive(Debug, Deserialize)]
struct HerdrTabResult {
    tab: HerdrTab,
}

#[derive(Debug, Deserialize)]
struct HerdrWorkspaceCreated {
    result: HerdrWorkspaceResult,
}

#[derive(Debug, Deserialize)]
struct HerdrWorkspaceResult {
    root_pane: HerdrRootPane,
    workspace: HerdrWorkspace,
}

/// Pick the multiplexer for an unset `--multiplexer`.
///
/// herdr when its server is reachable, else tmux. An explicit value is parsed
/// by [`Multiplexer::parse`] and always wins; this only decides the
/// unspecified case.
#[must_use]
pub const fn default_multiplexer(herdr_available: bool) -> Multiplexer {
    if herdr_available {
        Multiplexer::Herdr
    } else {
        Multiplexer::Tmux
    }
}

/// Derive a unique, human-recognizable session name from `cwd` and `now`.
///
/// The name is `<basename>-<hash>`: the cwd's sanitized basename (multiplexer
/// session names cannot carry `.`/`:`/spaces) plus a short hex hash of the
/// full path and timestamp. `now` is a parameter so the name is deterministic
/// under test; the caller passes [`chrono::Utc::now`], whose sub-second
/// precision makes two consecutive launches produce two distinct names.
#[must_use]
pub fn session_name(cwd: &Path, now: DateTime<Utc>) -> String {
    let raw_base = cwd
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("remargin");
    let base = sanitize_base(raw_base);

    let mut hasher = Sha256::new();
    hasher.update(cwd.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(now.to_rfc3339_opts(SecondsFormat::Nanos, true).as_bytes());
    let digest = hasher.finalize();

    let mut short = String::with_capacity(8);
    for byte in digest.iter().take(4) {
        let _ = write!(short, "{byte:02x}");
    }
    format!("{base}-{short}")
}

/// Replace every character that is not ASCII alphanumeric, `_`, or `-` with
/// `_`, so the result is safe as a tmux/herdr session name.
fn sanitize_base(base: &str) -> String {
    base.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Construct the full tmux launch plan for `session_name` over `tabs`.
///
/// The first tab is created with `new-session -d`; each further tab is a
/// `new-window` in the same session. Every tab's seed choreography targets
/// its window by name (`<session>:<identity>`), which is robust against the
/// user's `base-index` setting. Pure: builds argv vectors, spawns nothing.
#[must_use]
pub fn build_tmux_plan(session_name: &str, tabs: &[Tab]) -> TmuxPlan {
    let mut launch = Vec::with_capacity(tabs.len());
    let mut seeds = Vec::with_capacity(tabs.len());

    for (index, tab) in tabs.iter().enumerate() {
        let cwd = tab.cwd.to_string_lossy();
        let mut argv = if index == 0 {
            vec![
                "tmux".to_owned(),
                "new-session".to_owned(),
                "-d".to_owned(),
                "-s".to_owned(),
                session_name.to_owned(),
            ]
        } else {
            vec![
                "tmux".to_owned(),
                "new-window".to_owned(),
                "-t".to_owned(),
                session_name.to_owned(),
            ]
        };
        argv.push("-n".to_owned());
        argv.push(tab.identity.clone());
        argv.push("-c".to_owned());
        argv.push(cwd.into_owned());
        argv.push("--".to_owned());
        argv.extend(tab.launch_argv.iter().cloned());
        launch.push(argv);

        seeds.push(tmux_tab_seed(session_name, tab));
    }

    TmuxPlan {
        launch,
        tabs: seeds,
    }
}

/// The window target for send-keys/capture-pane: `<session>:<identity>`.
fn tmux_target(session_name: &str, identity: &str) -> String {
    format!("{session_name}:{identity}")
}

/// Build one tab's seed choreography (capture / trust-dismiss / seed lines).
fn tmux_tab_seed(session_name: &str, tab: &Tab) -> TmuxTabSeed {
    let target = tmux_target(session_name, &tab.identity);
    let capture = vec![
        "tmux".to_owned(),
        "capture-pane".to_owned(),
        "-t".to_owned(),
        target.clone(),
        "-p".to_owned(),
    ];
    let dismiss_trust = vec![
        "tmux".to_owned(),
        "send-keys".to_owned(),
        "-t".to_owned(),
        target.clone(),
        "Enter".to_owned(),
    ];
    let mut seed_lines = Vec::with_capacity(tab.seed_inputs.len() * 2);
    for line in &tab.seed_inputs {
        seed_lines.push(vec![
            "tmux".to_owned(),
            "send-keys".to_owned(),
            "-t".to_owned(),
            target.clone(),
            "-l".to_owned(),
            line.clone(),
        ]);
        seed_lines.push(vec![
            "tmux".to_owned(),
            "send-keys".to_owned(),
            "-t".to_owned(),
            target.clone(),
            "Enter".to_owned(),
        ]);
    }
    TmuxTabSeed {
        capture,
        dismiss_trust,
        identity: tab.identity.clone(),
        seed_lines,
    }
}

/// Construct the full herdr launch plan for `session_name` over `tabs`.
///
/// One `workspace create` for the session (rooted at the first tab's cwd),
/// then one tab per identity: identity 0 reuses the workspace's default tab
/// (no `tab create`), identities 1..N each get their own `tab create` carrying
/// a `<WS>` placeholder. Every identity's `agent start` targets its tab via a
/// `<TAB>` placeholder, followed by the wait-based readiness choreography and
/// the ordered name-addressed seed commands (both carrying a `<PANE>`
/// placeholder). Pure: builds argv vectors and spawns nothing; the thin
/// execution layer substitutes the real ids parsed from herdr's JSON.
#[must_use]
pub fn build_herdr_plan(session_name: &str, tabs: &[Tab]) -> HerdrPlan {
    let root = tabs.first().map_or_else(
        || ".".to_owned(),
        |tab| tab.cwd.to_string_lossy().into_owned(),
    );
    let create_workspace = vec![
        "herdr".to_owned(),
        "workspace".to_owned(),
        "create".to_owned(),
        "--cwd".to_owned(),
        root,
        "--label".to_owned(),
        session_name.to_owned(),
        "--no-focus".to_owned(),
    ];
    HerdrPlan {
        create_workspace,
        tabs: tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| herdr_tab_plan(index, tab))
            .collect(),
    }
}

/// Build one tab's herdr choreography: the `tab create` argv (empty for
/// identity 0, which reuses the workspace's default tab), the `agent start`
/// argv (with a `<TAB>` placeholder), the wait-based readiness sequence, and
/// the ordered name-addressed seed commands.
fn herdr_tab_plan(index: usize, tab: &Tab) -> HerdrTabPlan {
    let cwd = tab.cwd.to_string_lossy().into_owned();
    let tab_create = if index == 0 {
        Vec::new()
    } else {
        vec![
            "herdr".to_owned(),
            "tab".to_owned(),
            "create".to_owned(),
            "--workspace".to_owned(),
            HERDR_WORKSPACE_PLACEHOLDER.to_owned(),
            "--cwd".to_owned(),
            cwd.clone(),
            "--label".to_owned(),
            tab.identity.clone(),
            "--no-focus".to_owned(),
        ]
    };
    let mut agent_start = vec![
        "herdr".to_owned(),
        "agent".to_owned(),
        "start".to_owned(),
        tab.identity.clone(),
        "--tab".to_owned(),
        HERDR_TAB_PLACEHOLDER.to_owned(),
        "--cwd".to_owned(),
        cwd,
        "--no-focus".to_owned(),
        "--".to_owned(),
    ];
    agent_start.extend(tab.launch_argv.iter().cloned());

    let wait_trust = vec![
        "herdr".to_owned(),
        "wait".to_owned(),
        "output".to_owned(),
        HERDR_PANE_PLACEHOLDER.to_owned(),
        "--match".to_owned(),
        "trust".to_owned(),
        "--timeout".to_owned(),
        HERDR_TRUST_TIMEOUT_MS.to_string(),
    ];
    let wait_idle = vec![
        "herdr".to_owned(),
        "wait".to_owned(),
        "agent-status".to_owned(),
        HERDR_PANE_PLACEHOLDER.to_owned(),
        "--status".to_owned(),
        "idle".to_owned(),
        "--timeout".to_owned(),
        HERDR_IDLE_TIMEOUT_MS.to_string(),
    ];

    let mut seed = Vec::with_capacity(tab.seed_inputs.len() * 2);
    for line in &tab.seed_inputs {
        seed.push(vec![
            "herdr".to_owned(),
            "agent".to_owned(),
            "send".to_owned(),
            tab.identity.clone(),
            line.clone(),
        ]);
        seed.push(herdr_send_enter());
    }

    HerdrTabPlan {
        agent_start,
        dismiss_trust_enter: herdr_send_enter(),
        identity: tab.identity.clone(),
        seed,
        tab_create,
        wait_idle,
        wait_trust,
    }
}

/// `herdr pane send-keys <PANE> enter` — the submit after a seed line and the
/// trust-dialog dismissal.
fn herdr_send_enter() -> Vec<String> {
    vec![
        "herdr".to_owned(),
        "pane".to_owned(),
        "send-keys".to_owned(),
        HERDR_PANE_PLACEHOLDER.to_owned(),
        "enter".to_owned(),
    ]
}

/// Extract the workspace's default `tab_id` from a `herdr workspace create`
/// response's `root_pane` — the tab identity 0 reuses.
///
/// # Errors
///
/// Returns an error when `json` is not the expected `herdr workspace create`
/// shape.
fn parse_default_tab_id(json: &str) -> Result<String> {
    let parsed: HerdrWorkspaceCreated =
        serde_json::from_str(json).context("parsing 'herdr workspace create' JSON")?;
    Ok(parsed.result.root_pane.tab_id)
}

/// Extract the agent's `pane_id` from a `herdr agent start` response.
///
/// # Errors
///
/// Returns an error when `json` is not the expected `herdr agent start` shape.
fn parse_pane_id(json: &str) -> Result<String> {
    let parsed: HerdrAgentStarted =
        serde_json::from_str(json).context("parsing 'herdr agent start' JSON")?;
    Ok(parsed.result.agent.pane_id)
}

/// Extract the `(pane_id, tab_id)` entries from a `herdr pane list` response.
///
/// # Errors
///
/// Returns an error when `json` is not the expected `herdr pane list` shape.
fn parse_panes(json: &str) -> Result<Vec<HerdrPaneEntry>> {
    let parsed: HerdrPaneList =
        serde_json::from_str(json).context("parsing 'herdr pane list' JSON")?;
    Ok(parsed.result.panes)
}

/// Extract the new `tab_id` from a `herdr tab create` response.
///
/// # Errors
///
/// Returns an error when `json` is not the expected `herdr tab create` shape.
fn parse_tab_id(json: &str) -> Result<String> {
    let parsed: HerdrTabCreated =
        serde_json::from_str(json).context("parsing 'herdr tab create' JSON")?;
    Ok(parsed.result.tab.tab_id)
}

/// Extract the `workspace_id` from a `herdr workspace create` response.
///
/// # Errors
///
/// Returns an error when `json` is not the expected `herdr workspace create`
/// shape.
fn parse_workspace_id(json: &str) -> Result<String> {
    let parsed: HerdrWorkspaceCreated =
        serde_json::from_str(json).context("parsing 'herdr workspace create' JSON")?;
    Ok(parsed.result.workspace.workspace_id)
}

/// True when captured pane content shows the workspace-trust dialog.
///
/// Heuristic, used only by the ungated execution layer: the first
/// interactive `claude` launch in an untrusted folder prompts "Is this a
/// project you trust?".
fn pane_shows_trust_dialog(pane: &str) -> bool {
    pane.to_lowercase().contains("trust")
}

/// True when captured pane content shows the interactive `claude` prompt is
/// ready for input.
///
/// Heuristic, used only by the ungated execution layer: the live `claude`
/// TUI shows a shortcuts hint in its footer once it is accepting input.
fn pane_shows_ready_prompt(pane: &str) -> bool {
    pane.contains("for shortcuts")
}

/// Create a new named multiplexer session, one tab per identity, and seed
/// each with its `/loop` + `/goal` lines.
///
/// remargin exits after this: it writes no PID/registry file and does not
/// supervise the session. This is the thin execution layer — it spawns real
/// processes and is not exercised by the quality gate.
///
/// # Errors
///
/// Returns an error when `tabs` is empty, when the herdr preflight fails
/// (herdr selected but its server is unreachable), or when spawning a
/// multiplexer command fails.
pub fn launch_into_multiplexer(mux: Multiplexer, session_name: &str, tabs: &[Tab]) -> Result<()> {
    if tabs.is_empty() {
        bail!("no sessions to launch");
    }
    match mux {
        Multiplexer::Herdr => run_herdr_plan(session_name, tabs),
        Multiplexer::Tmux => run_tmux_plan(&build_tmux_plan(session_name, tabs)),
    }
}

/// True when `herdr status` reports a running, reachable server.
///
/// Spawns a real `herdr`; used for default-multiplexer selection and is not
/// exercised by the quality gate.
#[must_use]
pub fn herdr_available() -> bool {
    Command::new("herdr")
        .arg("status")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// The user-facing error when herdr cannot be used, naming the fix.
fn herdr_unavailable_error() -> anyhow::Error {
    anyhow::anyhow!(
        "herdr is not available: `herdr status` failed. Start the herdr server (`herdr`), \
         install herdr, or run with --multiplexer tmux"
    )
}

/// Fail early when herdr is unusable, with the fix in the message.
fn herdr_preflight() -> Result<()> {
    if herdr_available() {
        Ok(())
    } else {
        Err(herdr_unavailable_error())
    }
}

/// Execute a herdr plan: preflight, create the workspace, then per identity
/// place it in its own tab (reusing the workspace's default tab for identity 0,
/// creating a fresh tab for identities 1..N), start the agent, wait for
/// readiness (dismissing the trust dialog), and seed it — substituting the real
/// `workspace_id`/`tab_id`/`pane_id` parsed from herdr's JSON into the plan's
/// placeholder tokens. Thin execution: spawns real `herdr` processes and is not
/// exercised by the quality gate.
fn run_herdr_plan(session_name: &str, tabs: &[Tab]) -> Result<()> {
    herdr_preflight()?;
    let plan = build_herdr_plan(session_name, tabs);
    let workspace_json = capture_json(&plan.create_workspace)?;
    let workspace_id = parse_workspace_id(&workspace_json)?;
    let default_tab_id = parse_default_tab_id(&workspace_json)?;
    for tab in &plan.tabs {
        let tab_id = if tab.tab_create.is_empty() {
            default_tab_id.clone()
        } else {
            let tab_create =
                substitute(&tab.tab_create, HERDR_WORKSPACE_PLACEHOLDER, &workspace_id);
            let tab_json = capture_json(&tab_create)?;
            parse_tab_id(&tab_json)?
        };
        let agent_start = substitute(&tab.agent_start, HERDR_TAB_PLACEHOLDER, &tab_id);
        let agent_json = capture_json(&agent_start)?;
        let pane_id = parse_pane_id(&agent_json)?;
        // The trust dialog only appears on the first launch in an untrusted
        // folder. Probe for it best-effort: if it shows, dismiss it; if the
        // probe times out (already trusted), do NOT fail the launch — an early
        // draft did, which aborted every later agent. Readiness is `wait_idle`.
        if run_command_ok(&substitute(
            &tab.wait_trust,
            HERDR_PANE_PLACEHOLDER,
            &pane_id,
        )) {
            run_command(&substitute(
                &tab.dismiss_trust_enter,
                HERDR_PANE_PLACEHOLDER,
                &pane_id,
            ))?;
        }
        run_command(&substitute(
            &tab.wait_idle,
            HERDR_PANE_PLACEHOLDER,
            &pane_id,
        ))?;
        for argv in &tab.seed {
            run_command(&substitute(argv, HERDR_PANE_PLACEHOLDER, &pane_id))?;
        }
        // `agent start --tab` splits a NEW pane for the agent, leaving the
        // tab's original pane empty. Close every non-agent pane in the tab so
        // each tab holds only its agent. Best-effort: cosmetic cleanup that
        // must never abort a launch whose agents are already up.
        close_stray_panes(&workspace_id, &tab_id, &pane_id);
    }
    Ok(())
}

/// Best-effort: close every pane in `tab_id` except the agent's `keep` pane.
/// `agent start --tab` splits a new pane, leaving the tab's default pane empty;
/// this removes it. Spawns real `herdr` and is not exercised by the gate; every
/// step is tolerant of failure so cosmetic cleanup never aborts a live launch.
fn close_stray_panes(workspace_id: &str, tab_id: &str, keep: &str) {
    let list = vec![
        "herdr".to_owned(),
        "pane".to_owned(),
        "list".to_owned(),
        "--workspace".to_owned(),
        workspace_id.to_owned(),
    ];
    let Ok(json) = capture_json(&list) else {
        return;
    };
    let Ok(panes) = parse_panes(&json) else {
        return;
    };
    for entry in panes {
        if entry.tab_id == tab_id && entry.pane_id != keep {
            run_command_ok(&[
                "herdr".to_owned(),
                "pane".to_owned(),
                "close".to_owned(),
                entry.pane_id,
            ]);
        }
    }
}

/// Replace every argv element equal to `placeholder` with `value`.
fn substitute(argv: &[String], placeholder: &str, value: &str) -> Vec<String> {
    argv.iter()
        .map(|part| {
            if part == placeholder {
                value.to_owned()
            } else {
                part.clone()
            }
        })
        .collect()
}

/// Spawn a tmux plan's launch commands, then seed each tab once its TUI is
/// ready.
fn run_tmux_plan(plan: &TmuxPlan) -> Result<()> {
    for argv in &plan.launch {
        run_command(argv)?;
    }
    for tab in &plan.tabs {
        seed_tmux_tab(tab)?;
    }
    Ok(())
}

/// Poll the tab's pane for the trust dialog (dismiss it) and then the ready
/// prompt, up to [`READINESS_MAX_POLLS`], before typing its seed lines.
/// Seeding proceeds even if readiness is never observed: remargin starts the
/// session best-effort and never tears it down, so a still-attachable session
/// is preferable to bailing.
fn seed_tmux_tab(tab: &TmuxTabSeed) -> Result<()> {
    let mut dismissed = false;
    for _ in 0..READINESS_MAX_POLLS {
        let pane = capture_pane(&tab.capture)?;
        if !dismissed && pane_shows_trust_dialog(&pane) {
            run_command(&tab.dismiss_trust)?;
            dismissed = true;
            sleep(READINESS_POLL);
            continue;
        }
        if pane_shows_ready_prompt(&pane) {
            break;
        }
        sleep(READINESS_POLL);
    }
    for argv in &tab.seed_lines {
        run_command(argv)?;
    }
    Ok(())
}

/// Run a constructed command, failing if it cannot be spawned or exits
/// non-zero.
fn run_command(argv: &[String]) -> Result<()> {
    let (program, rest) = argv
        .split_first()
        .context("multiplexer command argv is empty")?;
    let status = Command::new(program)
        .args(rest)
        .status()
        .with_context(|| format!("spawning {program:?} -- is it installed?"))?;
    if !status.success() {
        bail!("command {program:?} exited with {status}");
    }
    Ok(())
}

/// Run a constructed command best-effort, returning whether it exited zero.
/// Never fails the launch — used for the trust-dialog probe, whose timeout
/// (exit 1) just means the folder was already trusted.
fn run_command_ok(argv: &[String]) -> bool {
    let Some((program, rest)) = argv.split_first() else {
        return false;
    };
    Command::new(program)
        .args(rest)
        .status()
        .is_ok_and(|status| status.success())
}

/// Capture a tmux pane's visible content for readiness polling.
fn capture_pane(argv: &[String]) -> Result<String> {
    let (program, rest) = argv.split_first().context("capture-pane argv is empty")?;
    let output = Command::new(program)
        .args(rest)
        .output()
        .with_context(|| format!("spawning {program:?} -- is it installed?"))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a herdr command and capture its stdout JSON, failing on a non-zero exit.
fn capture_json(argv: &[String]) -> Result<String> {
    let (program, rest) = argv.split_first().context("herdr command argv is empty")?;
    let output = Command::new(program)
        .args(rest)
        .output()
        .with_context(|| format!("spawning {program:?} -- is it installed?"))?;
    if !output.status.success() {
        bail!("command {program:?} exited with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
#[path = "multiplexer_tests.rs"]
mod multiplexer_tests;

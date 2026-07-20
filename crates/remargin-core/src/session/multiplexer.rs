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
//! - **Pure construction** ([`session_name`], [`build_tmux_plan`]) turns a
//!   `(session_name, tabs)` pair into the exact tmux command argv vectors and
//!   the full trust-dismiss + seed send-keys sequence. These are asserted
//!   command-for-command in the tests.
//! - **Thin execution** ([`launch_into_multiplexer`]) spawns the constructed
//!   commands via [`std::process::Command`] and does the bounded readiness
//!   polling between launching a tab and seeding it. It is deliberately kept
//!   out of the quality gate: it needs a real tmux/`claude` and would be
//!   flaky, so nothing here calls it.
//!
//! zellij is parsed and modelled but its launch path is gated: a live spike
//! (see the `session-launch` bead this task filed) found the spec's preferred
//! zellij mechanism unworkable as written, so `--multiplexer zellij` returns
//! a clear pending error rather than a half-working launch. tmux is the
//! default and fully works.

use core::fmt::Write as _;
use core::time::Duration;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use sha2::{Digest as _, Sha256};

/// The bead tracking the deferred zellij launch path. Surfaced verbatim in
/// the `--multiplexer zellij` pending error so the operator can find it.
const ZELLIJ_PENDING_BEAD: &str = "rem-1x1t";

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
    /// tmux — the default; fully implemented.
    Tmux,
    /// zellij — parsed but its launch path is currently gated.
    Zellij,
}

impl Multiplexer {
    /// A human-facing hint for reattaching to `session_name`.
    #[must_use]
    pub fn attach_hint(self, session_name: &str) -> String {
        match self {
            Self::Tmux => format!("tmux attach -t {session_name}"),
            Self::Zellij => format!("zellij attach {session_name}"),
        }
    }

    /// Stable lowercase name (`"tmux"` / `"zellij"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Zellij => "zellij",
        }
    }

    /// Parse the `--multiplexer` value.
    ///
    /// # Errors
    ///
    /// Returns an error naming the allowed values when `value` is neither
    /// `tmux` nor `zellij`.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "tmux" => Ok(Self::Tmux),
            "zellij" => Ok(Self::Zellij),
            other => bail!("unknown multiplexer {other:?}; allowed: tmux, zellij"),
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
/// `_`, so the result is safe as a tmux/zellij session name.
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
/// Returns an error when `tabs` is empty, when the selected multiplexer's
/// launch path is not yet available (zellij), or when spawning a tmux
/// command fails.
pub fn launch_into_multiplexer(mux: Multiplexer, session_name: &str, tabs: &[Tab]) -> Result<()> {
    if tabs.is_empty() {
        bail!("no sessions to launch");
    }
    match mux {
        Multiplexer::Tmux => run_tmux_plan(&build_tmux_plan(session_name, tabs)),
        Multiplexer::Zellij => bail!(
            "zellij support pending {ZELLIJ_PENDING_BEAD}; run with the default --multiplexer tmux"
        ),
    }
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

/// Capture a tmux pane's visible content for readiness polling.
fn capture_pane(argv: &[String]) -> Result<String> {
    let (program, rest) = argv.split_first().context("capture-pane argv is empty")?;
    let output = Command::new(program)
        .args(rest)
        .output()
        .with_context(|| format!("spawning {program:?} -- is it installed?"))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
#[path = "multiplexer_tests.rs"]
mod multiplexer_tests;

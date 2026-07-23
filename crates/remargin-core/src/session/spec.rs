//! Launch-spec builder for `remargin session launch`.
//!
//! [`build_launch_spec`] turns a [`DiscoveredSession`] (task 82) into
//! everything the backend (task 85) needs to bring up that identity's
//! interactive `claude` session: its working directory, a `remargin mcp`
//! server scoped to that directory + identity, the composed system prompt,
//! and the backend params (model / effort / budget). This is also where
//! `goal` graduates from an optional config field to a hard launch
//! requirement, and `loop` picks up its [`DEFAULT_LOOP`] cadence when unset.
//!
//! Pure builder: it composes and validates, it never spawns a process or
//! writes to disk. The `/loop` interval and `/goal` condition are kept as
//! separate structured fields (`loop_interval`, `goal`) because task 85
//! renders them as separate interactive slash-command submissions via the
//! multiplexer's send-keys; [`compose_prompt`] additionally folds their
//! framing into the prompt text so the composed prompt is self-describing.

use core::time::Duration;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

use super::discovery::DiscoveredSession;
use crate::config::Budget;
use crate::config::SessionConfig;
use crate::config::system_prompt::ResolvedSystemPrompt;

/// `/loop` cadence used when neither the agent's `session:` block nor a
/// manifest entry declares one. Settled default: 5 minutes.
pub const DEFAULT_LOOP: Duration = Duration::from_secs(300);

/// The standard remargin operating rules folded into every launched
/// session's system prompt, below the resolved `system_prompt:` body.
const REMARGIN_OPERATING_RULES: &str = "\
# Remargin operating rules

- Run `remargin activity` on your realm before processing comments -- the \
pending queue is only one signal.
- Work the comments addressed to you via the `remargin` MCP: reply through \
the thread, address people with `to:` so they see it in their pending queue, \
and ack only your own queue.
- Surface decisions you cannot make as comments to the owner, not as prose \
left in the document body.
- Respect the realm's permissions and signing mode; never weaken a gate to \
make an operation succeed.";

/// How to bring up the `remargin mcp` server for one launched session.
///
/// Carries the argv rather than a running server: task 85 decides whether
/// to attach it inline (`--mcp-config`) or register it (`claude mcp add`),
/// per task 81's finding.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct McpServerSpec {
    /// argv for the scoped `remargin mcp` server (`["remargin", "mcp"]`).
    pub argv: Vec<String>,
    /// Directory the server is scoped to — the session's `cwd`.
    pub base_dir: PathBuf,
    /// Identity the server runs as.
    pub identity: String,
}

/// Everything required to launch one identity's session, assembled and
/// validated. Produced by [`build_launch_spec`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SessionLaunchSpec {
    /// Per-session resource caps. `None` = no cap.
    pub budget: Option<Budget>,
    /// Working directory the session launches in.
    pub cwd: PathBuf,
    /// `--effort` for the backend, from `session.claude`.
    pub effort: Option<String>,
    /// `/goal` stop condition — required, validated at build time.
    pub goal: String,
    /// Identity governing this session.
    pub identity: String,
    /// `/loop` cadence — the declared value, or [`DEFAULT_LOOP`] when unset.
    pub loop_interval: Duration,
    /// The scoped `remargin mcp` server to bring up.
    pub mcp: McpServerSpec,
    /// `--model` for the backend, from `session.claude`.
    pub model: Option<String>,
    /// System prompt body + remargin operating rules + `/loop` + `/goal`,
    /// composed. The authoritative machine-usable cadence and goal live in
    /// [`Self::loop_interval`] / [`Self::goal`]; this string carries their
    /// framing as text.
    pub prompt: String,
}

/// Assemble and validate the launch spec for one discovered session.
///
/// `goal` is the one hard launch requirement: this is the authoritative
/// enforcement behind task 83's soft dry-run flag. `loop` defaults to
/// [`DEFAULT_LOOP`] when unset, so an absent `session:` block is treated as
/// an empty one and fails naming `goal`. `budget == None` passes through as
/// "no cap"; `model` / `effort` flow from `session.claude`.
///
/// # Errors
///
/// Returns an error naming the identity and the offending field when the
/// session's `loop` is set but unparseable, or its `goal` is unset.
pub fn build_launch_spec(session: &DiscoveredSession) -> Result<SessionLaunchSpec> {
    // An absent block is an empty block: `goal` is the one hard requirement.
    let empty = SessionConfig::default();
    let s = session.session.as_ref().unwrap_or(&empty);
    let loop_interval = s
        .loop_duration()
        .with_context(|| format!("identity {:?}: bad `loop`", session.identity))?
        .unwrap_or(DEFAULT_LOOP);
    let goal = s.goal.clone().with_context(|| {
        format!(
            "identity {:?}: `goal` is required to launch",
            session.identity
        )
    })?;

    let (model, effort) = s
        .claude
        .as_ref()
        .map_or((None, None), |c| (c.model.clone(), c.effort.clone()));

    let prompt = compose_prompt(&session.system_prompt, loop_interval, &goal);

    Ok(SessionLaunchSpec {
        budget: s.budget.clone(),
        cwd: session.folder.clone(),
        effort,
        goal,
        identity: session.identity.clone(),
        loop_interval,
        mcp: McpServerSpec {
            argv: vec!["remargin".to_owned(), "mcp".to_owned()],
            base_dir: session.folder.clone(),
            identity: session.identity.clone(),
        },
        model,
        prompt,
    })
}

/// Compose the launched session's prompt.
///
/// Concatenates the resolved system-prompt body, the standard remargin
/// operating rules, and the `/loop` + `/goal` framing (whose authoritative
/// values remain the spec's structured `loop_interval` / `goal` fields).
#[must_use]
pub fn compose_prompt(
    system_prompt: &ResolvedSystemPrompt,
    loop_interval: Duration,
    goal: &str,
) -> String {
    format!(
        "{body}\n\n{REMARGIN_OPERATING_RULES}\n\n/loop {interval}\n/goal {goal}",
        body = system_prompt.prompt,
        interval = humantime::format_duration(loop_interval),
    )
}

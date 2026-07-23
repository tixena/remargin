//! Session backends for `remargin session launch`.
//!
//! A backend renders a validated [`SessionLaunchSpec`] into what a
//! multiplexer tab needs to bring one identity's session up. Per task 81's
//! verified findings that is two things, not one: an **interactive** launch
//! command (argv) that starts the session, and the `/loop` + `/goal`
//! slash-command lines that are *typed into* the already-running session via
//! the multiplexer's send-keys. Rendering only -- starting the session and
//! typing the seeds is task 86's job, and `remargin` never ends, reaps, or
//! otherwise babysits the session it starts (design decision 2).

use anyhow::{Context as _, Result, bail};
use serde_json::json;

use super::spec::SessionLaunchSpec;

/// Renders a launch spec into a runnable session: the interactive launch
/// argv plus the slash-command lines to seed the loop and goal once it is
/// live. Both halves are rendering, not spawning.
pub trait SessionBackend {
    /// The interactive launch argv. Starts the session and seeds nothing;
    /// the `/loop` + `/goal` lines are delivered separately via
    /// [`Self::seed_inputs`].
    ///
    /// # Errors
    ///
    /// Returns an error when the spec cannot be rendered into a command.
    fn launch_command(&self, spec: &SessionLaunchSpec) -> Result<Vec<String>>;
    /// Stable backend identifier (e.g. `"claude"`).
    fn name(&self) -> &'static str;
    /// Slash-command lines to type into the running session to start the
    /// loop and set the goal (e.g. `["/loop 30s", "/goal ..."]`). Consumed
    /// by task 86's send-keys.
    fn seed_inputs(&self, spec: &SessionLaunchSpec) -> Vec<String>;
}

/// The `claude` backend: an **interactive** `claude` session per task 81's
/// verified invocation (v2.1.215), seeded with `/loop` + `/goal` through the
/// multiplexer once the TUI is live.
#[non_exhaustive]
pub struct ClaudeBackend;

impl SessionBackend for ClaudeBackend {
    fn launch_command(&self, spec: &SessionLaunchSpec) -> Result<Vec<String>> {
        let (mcp_command, mcp_args) = spec
            .mcp
            .argv
            .split_first()
            .context("session mcp argv is empty")?;
        // Inline, `--strict-mcp-config`-scoped remargin server (task 81):
        // no global `claude mcp add` is required.
        let mcp_config = json!({
            "mcpServers": { "remargin": { "command": mcp_command, "args": mcp_args } }
        })
        .to_string();

        let mut argv = vec![
            "claude".to_owned(),
            "--append-system-prompt".to_owned(),
            spec.prompt.clone(),
            "--mcp-config".to_owned(),
            mcp_config,
            "--strict-mcp-config".to_owned(),
        ];
        if let Some(model) = &spec.model {
            argv.push("--model".to_owned());
            argv.push(model.clone());
        }
        if let Some(effort) = &spec.effort {
            argv.push("--effort".to_owned());
            argv.push(effort.clone());
        }
        argv.push("-n".to_owned());
        argv.push(spec.identity.clone());
        argv.push("--permission-mode".to_owned());
        // `auto`, not `acceptEdits`: an unattended loop agent must be able to
        // call the remargin MCP tools without stalling on a prompt, and
        // `acceptEdits` only auto-approves file edits, not MCP tool calls.
        argv.push("auto".to_owned());
        // `budget.max_turns` rides in the `/goal` seed line (`seed_inputs`);
        // interactive `claude` has no budget flags.
        Ok(argv)
    }

    fn name(&self) -> &'static str {
        "claude"
    }

    fn seed_inputs(&self, spec: &SessionLaunchSpec) -> Vec<String> {
        let interval = humantime::format_duration(spec.loop_interval);
        let goal = spec
            .budget
            .as_ref()
            .and_then(|budget| budget.max_turns)
            .map_or_else(
                || format!("/goal {}", spec.goal),
                |max_turns| format!("/goal {} or stop after {max_turns} turns", spec.goal),
            );
        vec![format!("/loop {interval}"), goal]
    }
}

/// Resolve a backend by name.
///
/// # Errors
///
/// Returns an error naming the known backends when `name` is not one of
/// them.
pub fn resolve_backend(name: &str) -> Result<Box<dyn SessionBackend>> {
    match name {
        "claude" => Ok(Box::new(ClaudeBackend)),
        other => bail!("unknown backend {other:?}; known: claude"),
    }
}

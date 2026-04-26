//! Claude-settings synchronizer rule generation (rem-yj1j.4 / T25,
//! slice 1 — `rem-wv71`).
//!
//! [`rules_for`] is a pure function over a [`ResolvedRestrict`] +
//! anchor + `allow_dot_folders` list. Given those inputs it produces
//! the exact `permissions.deny` / `permissions.allow` rule strings
//! that the Claude-settings merger (slice 3, `rem-7m4u`) will write
//! into `.claude/settings.local.json` and `~/.claude/settings.json`.
//!
//! ## Output shape
//!
//! ```text
//! deny:
//!   Edit(//<path>/**)
//!   Write(//<path>/**)
//!   Read(//<path>/**)
//!   NotebookEdit(//<path>/**)
//!   Read(//<path>/.*/**)             ← dot-folder default-deny (one
//!   Edit(//<path>/.*/**)               wildcard rule per Claude tool;
//!   Write(//<path>/.*/**)              suppressed when allow_dot_folders
//!   NotebookEdit(//<path>/.*/**)       names every dot-folder)
//!   <per allow_dot_folders entry, RE-allow rules>
//!   Bash(cp * //<path>/**)            ← write-side bash mutators
//!   Bash(mv * //<path>/**)
//!   Bash(tee //<path>/**)
//!   Bash(sed -i * //<path>/**)
//!   Bash(truncate * //<path>/**)
//!   Bash(touch //<path>/**)
//!   <per also_deny_bash entry, Bash(<cmd> * //<path>/**)>
//!   Bash(remargin * //<path>/**)      ← only when cli_allowed=false
//!
//! allow:
//!   mcp__remargin__*                  ← always present
//! ```
//!
//! ## Why a single wildcard for dot-folder denies
//!
//! The spec proposed two options: enumerate every `.<name>/` under the
//! path, or emit one wildcard `.*` rule. Walking the filesystem at
//! rule-generation time is expensive AND races against folder
//! creation. A single `.*/**` wildcard rule covers all current and
//! future dot-folders without filesystem access. When
//! `allow_dot_folders` lists specific names that should remain
//! reachable (e.g. `.github`), we add narrow re-allows that override
//! the broader deny — Claude's permission resolution gives the more-
//! specific allow precedence.
//!
//! ## `.remargin/` is always allowed
//!
//! Remargin owns the `.remargin/` folder (its own state directory).
//! Even if `allow_dot_folders` does not list it, this module emits an
//! explicit re-allow for `.remargin/` so the runtime keeps working.
//!
//! ## No filesystem access
//!
//! Every input is materialised by the caller; this module produces
//! `Vec<String>` only. That keeps it trivially testable with
//! `MockSystem` not even needed — pure data in, pure data out.

#[cfg(test)]
mod tests;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::permissions::resolve::{ResolvedRestrict, RestrictPath};

/// The dot-folder remargin owns. Always re-allowed regardless of the
/// caller's `allow_dot_folders` list.
const REMARGIN_DOT_FOLDER: &str = ".remargin";

/// Editor-side Claude tools touched by the base path-deny and the
/// dot-folder default-deny. Order matches the spec's example output
/// (Edit / Write / Read / `NotebookEdit`) so settings-file diffs read
/// the way users expect.
const EDITOR_TOOLS: &[&str] = &["Edit", "Write", "Read", "NotebookEdit"];

/// Write-side Bash mutators that need their own deny rules to keep
/// shell-out paths from sneaking around the editor-tool denies.
/// Listed in the order the spec calls out.
const BASH_MUTATORS: &[&str] = &["cp *", "mv *", "tee", "sed -i *", "truncate *", "touch"];

/// Allow rule that pins remargin's MCP tools as always-callable so a
/// blanket `restrict` rule does not lock the user out of the very
/// commands needed to reverse it.
const ALLOW_MCP_REMARGIN: &str = "mcp__remargin__*";

/// Generated rule strings for one [`ResolvedRestrict`] entry.
///
/// `deny` and `allow` map 1:1 to Claude's `permissions.deny` /
/// `permissions.allow` arrays. Both sides of the sync (apply +
/// reverse) work off this exact set so the round-trip is exact.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RuleSet {
    /// `permissions.allow` rules. Always contains
    /// [`ALLOW_MCP_REMARGIN`] so the user can still call remargin
    /// tools even under a blanket restrict.
    pub allow: Vec<String>,
    /// `permissions.deny` rules in emit order.
    pub deny: Vec<String>,
}

/// Compute the rule set for one resolved restrict entry.
///
/// Pure: no filesystem access. The caller must pass the realm anchor
/// (the directory that holds `.claude/`) so wildcard entries can
/// expand to a concrete path glob. `allow_dot_folders` controls which
/// dot-folder names get a re-allow rule on top of the default-deny.
///
/// Wildcards (`RestrictPath::Wildcard`) anchor at the entry's
/// `realm_root`; `_anchor` is unused for these entries because the
/// realm root already anchors them. Absolute entries use their own
/// path verbatim.
#[must_use]
pub fn rules_for(
    entry: &ResolvedRestrict,
    _anchor: &Path,
    allow_dot_folders: &[String],
) -> RuleSet {
    let restricted_root = match &entry.path {
        RestrictPath::Absolute(path) => path.clone(),
        RestrictPath::Wildcard { realm_root } => realm_root.clone(),
    };
    let glob_root = restricted_root.display().to_string();

    let mut deny: Vec<String> = Vec::new();

    // 1. Base read/write tool denies — the editor-side defenses.
    for tool in EDITOR_TOOLS {
        deny.push(format!("{tool}(//{glob_root}/**)"));
    }

    // 2. Dot-folder default-deny. A single wildcard rule per tool
    //    covers every current and future dot-folder under the
    //    restricted root; specific allows below override.
    for tool in EDITOR_TOOLS {
        deny.push(format!("{tool}(//{glob_root}/.*/**)"));
    }

    // 3. Bash mutators — keep shell-out paths from dodging the
    //    editor-tool denies.
    for cmd in BASH_MUTATORS {
        deny.push(format!("Bash({cmd} //{glob_root}/**)"));
    }

    // 4. Caller-supplied bash extras, e.g. `also_deny_bash: [curl]`.
    for cmd in &entry.also_deny_bash {
        deny.push(format!("Bash({cmd} * //{glob_root}/**)"));
    }

    // 5. Block remargin CLI invocations against the restricted root
    //    unless the caller explicitly opted in via `cli_allowed: true`.
    if !entry.cli_allowed {
        deny.push(format!("Bash(remargin * //{glob_root}/**)"));
    }

    // 6. Allow list. The MCP allow is always present; per-dot-folder
    //    re-allows override the default-deny for explicitly opted-in
    //    folders, plus the always-allowed `.remargin/`.
    let mut allow: Vec<String> = vec![String::from(ALLOW_MCP_REMARGIN)];
    let mut allowed: Vec<&str> = Vec::with_capacity(allow_dot_folders.len() + 1);
    allowed.push(REMARGIN_DOT_FOLDER);
    for entry_name in allow_dot_folders {
        if entry_name != REMARGIN_DOT_FOLDER {
            allowed.push(entry_name.as_str());
        }
    }
    for folder in &allowed {
        for tool in EDITOR_TOOLS {
            allow.push(format!("{tool}(//{glob_root}/{folder}/**)"));
        }
    }

    RuleSet { allow, deny }
}

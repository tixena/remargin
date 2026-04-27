//! Structural decomposition of Claude permission rule strings used by
//! the `plan restrict` conflict detector (rem-aovx).
//!
//! The original detector compared rule bodies as raw strings. That
//! caught only exact-format matches: a hand-edited rule, a legacy
//! double-slash prefix, or a trailing-slash difference would silently
//! slip past. This module provides a structural parser so the detector
//! can compare rules by `(tool, path-glob)` shape and reason about
//! prefix / recursive-subtree overlap.
//!
//! Lossy by design: round-tripping back to a rule string is not
//! supported. Callers keep the original rule string for echo-back; this
//! struct exists for comparisons.
//!
//! Out of scope: full glob semantics across `?`, `[a-z]`, brace
//! expansion. Initial scope is prefix-overlap on the absolute path with
//! `/**` as the recursive-everything sentinel and cross-tool pairs kept
//! distinct.

/// Relationship between a projected deny and an existing allow.
///
/// Surfaced on
/// [`crate::operations::plan::ConfigConflict::AllowDenyOverlap`] so
/// the CLI / MCP can render a kind-specific message.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OverlapKind {
    /// The existing allow targets a sub-tree that the broader projected
    /// deny would shadow (e.g. allow `Read(/foo/sub)`, deny
    /// `Read(/foo/**)`).
    AllowShadowedByBroaderDeny,
    /// The projected deny targets a sub-tree of an already-broader
    /// existing allow (e.g. allow `Read(/foo/**)`, deny
    /// `Read(/foo/sub)`). The deny may not stick under most precedence
    /// rules.
    DenyShadowedByBroaderAllow,
    /// Allow and deny target the exact same path-glob.
    Exact,
}

/// Canonicalized representation of a path glob inside a permission
/// rule.
///
/// Stored as a `Vec<String>` of path components so prefix-overlap is a
/// Vec-prefix check, not a string-prefix check (avoids the classic
/// `/foo/bar` vs `/foobar` confusion).
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub struct PathGlob {
    /// Canonical path components. Empty means root-relative; first
    /// component is the first directory after the leading `/`.
    pub components: Vec<String>,
    /// `true` when the rule ended in `/**` (recursive subtree). When
    /// `false`, the rule targets a specific file/directory.
    pub recursive: bool,
}

impl PathGlob {
    /// Classify the overlap relationship between an existing `allow`
    /// glob (`self`) and a projected `deny` glob (`other`). Returns
    /// `None` when the two do not overlap.
    #[must_use]
    pub fn classify_overlap(&self, other: &Self) -> Option<OverlapKind> {
        if self.components == other.components {
            // Either both recursive, both non-recursive, or one of
            // them is recursive over the same path. All of these are
            // "exact" for overlap-classification purposes — the user
            // sees identical rule bodies.
            return Some(OverlapKind::Exact);
        }
        if other.components.len() < self.components.len()
            && self.components.starts_with(&other.components)
            && other.recursive
        {
            // The deny is the broader (shorter, recursive) side and
            // shadows the more-specific allow.
            return Some(OverlapKind::AllowShadowedByBroaderDeny);
        }
        if self.components.len() < other.components.len()
            && other.components.starts_with(&self.components)
            && self.recursive
        {
            // The allow is the broader (shorter, recursive) side; the
            // deny is shadowed by the broader allow.
            return Some(OverlapKind::DenyShadowedByBroaderAllow);
        }
        None
    }

    /// `true` when `self` overlaps `other`.
    ///
    /// Overlap rules:
    /// - Identical paths (recursive flag aside) overlap unconditionally.
    /// - One side's components are a prefix of the other AND that
    ///   prefix-side is `recursive` → overlap (the recursive side
    ///   covers the longer path).
    /// - Disjoint or component-confused paths (`/foo` vs `/foobar`) do
    ///   not overlap (component-wise comparison rules out string-prefix
    ///   confusion).
    /// - Same prefix but neither side recursive only overlaps when the
    ///   components are equal.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        if self.components == other.components {
            return true;
        }
        if self.components.len() < other.components.len()
            && other.components.starts_with(&self.components)
        {
            return self.recursive;
        }
        if other.components.len() < self.components.len()
            && self.components.starts_with(&other.components)
        {
            return other.recursive;
        }
        false
    }

    /// Parse the path-glob portion of a rule body.
    ///
    /// Collapses runs of `/`, lexically resolves `.` and `..`, strips
    /// trailing `/` and `/**`, records `recursive` accordingly.
    /// Whitespace around the glob is trimmed.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let trimmed = s.trim();
        let stripped = trimmed.strip_suffix("/**");
        let recursive = stripped.is_some() || trimmed == "**";
        let body_after_recursive: &str = stripped.unwrap_or(trimmed);
        // Strip a single trailing `/` (after `/**` removal). A bare
        // `/foo/` and `/foo` should compare equal.
        let cleaned = body_after_recursive
            .strip_suffix('/')
            .unwrap_or(body_after_recursive);

        let mut components: Vec<String> = Vec::new();
        for raw in cleaned.split('/') {
            if raw.is_empty() || raw == "." {
                continue;
            }
            if raw == ".." {
                if !components.is_empty() {
                    let _: Option<String> = components.pop();
                }
                continue;
            }
            components.push(String::from(raw));
        }
        Self {
            components,
            recursive,
        }
    }
}

/// Structural decomposition of a Claude permission rule string used by
/// the conflict detector. See module docs for caveats.
///
/// `String`-owned (rather than `&str`-borrowed) so it slots into the
/// detector without lifetime gymnastics. The cost is one allocation
/// per parsed rule; the detector runs on tens of rules per file, not
/// thousands.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum RuleShape {
    /// `Bash(<cmd-tokens>… <path-glob>)`. Cmd tokens kept verbatim
    /// (case-preserving); only the trailing path-glob is normalized.
    Bash {
        /// Whitespace-split tokens preceding the trailing path glob.
        cmd_tokens: Vec<String>,
        /// Canonicalized path glob (the last whitespace-separated
        /// token, parsed via [`PathGlob::parse`]).
        path_glob: PathGlob,
    },
    /// Any rule that does not match the canonical shapes above.
    /// Treated as opaque (skipped by the detector). Examples:
    /// `mcp__remargin__*`, `WebFetch(domain:github.com)`,
    /// `Bash(ls *)` (no path glob).
    Opaque(String),
    /// `Tool(<path-glob>)` for any tool whose body is a single path.
    Tool {
        /// Canonicalized path glob.
        path_glob: PathGlob,
        /// Literal tool name (`Read`, `Write`, `Edit`, `NotebookEdit`,
        /// …). Compared case-sensitively.
        tool: String,
    },
}

impl RuleShape {
    /// Parse a full rule string. Returns [`RuleShape::Opaque`] for any
    /// shape the detector does not understand structurally.
    #[must_use]
    pub fn parse(rule: &str) -> Self {
        let trimmed = rule.trim();
        let Some(open) = trimmed.find('(') else {
            return Self::Opaque(String::from(rule));
        };
        let Some(close_offset) = trimmed.rfind(')') else {
            return Self::Opaque(String::from(rule));
        };
        if close_offset <= open {
            return Self::Opaque(String::from(rule));
        }
        let tool = &trimmed[..open];
        let body = &trimmed[open + 1..close_offset];

        if tool == "Bash" {
            return parse_bash(rule, body);
        }
        // Heuristic: known editor tools whose body is a path glob.
        if matches!(tool, "Read" | "Write" | "Edit" | "NotebookEdit") {
            let path_glob = PathGlob::parse(body);
            return Self::Tool {
                path_glob,
                tool: String::from(tool),
            };
        }
        Self::Opaque(String::from(rule))
    }
}

fn parse_bash(rule: &str, body: &str) -> RuleShape {
    let trimmed_body = body.trim();
    if trimmed_body.is_empty() {
        return RuleShape::Opaque(String::from(rule));
    }
    let tokens: Vec<&str> = trimmed_body.split_whitespace().collect();
    let Some(last) = tokens.last() else {
        return RuleShape::Opaque(String::from(rule));
    };
    if !looks_like_path(last) {
        // No trailing path glob — fall back to opaque so we do not
        // pretend to understand the structure.
        return RuleShape::Opaque(String::from(rule));
    }
    let path_glob = PathGlob::parse(last);
    let cmd_tokens: Vec<String> = tokens[..tokens.len() - 1]
        .iter()
        .map(|t| String::from(*t))
        .collect();
    RuleShape::Bash {
        cmd_tokens,
        path_glob,
    }
}

fn looks_like_path(token: &str) -> bool {
    token.starts_with('/')
}

/// Compare two parsed rules for overlap. `Opaque` rules never overlap,
/// and cross-tool / cross-cmd-token pairs return `None`.
#[must_use]
pub fn rules_overlap(a: &RuleShape, b: &RuleShape) -> Option<OverlapKind> {
    match (a, b) {
        (
            RuleShape::Tool {
                path_glob: pa,
                tool: ta,
            },
            RuleShape::Tool {
                path_glob: pb,
                tool: tb,
            },
        ) if ta == tb => pa.classify_overlap(pb),
        (
            RuleShape::Bash {
                cmd_tokens: ca,
                path_glob: pa,
            },
            RuleShape::Bash {
                cmd_tokens: cb,
                path_glob: pb,
            },
        ) if ca == cb => pa.classify_overlap(pb),
        _ => None,
    }
}

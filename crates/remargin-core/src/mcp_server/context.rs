//! What every call of the generated server carries: the realm it runs in, and the guards each
//! handler runs first.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use os_shim::System;

use crate::config::ResolvedConfig;
use crate::config::permissions::resolve::ResolvedPermissions;
use crate::mcp::errors::{HumanIdentityRejected, RemarginError};
use crate::mcp::{HUMAN_IDENTITY_EXEMPT_TOOLS, check_one_path};
use crate::parser::AuthorType;
use crate::path::expand_path;

/// The realm one request runs in, resolved afresh for each request.
#[non_exhaustive]
pub struct RequestContext<'request> {
    pub base_dir: &'request Path,
    pub config: ResolvedConfig,
    pub permissions: ResolvedPermissions,
    pub system: &'request dyn System,
}

impl RequestContext<'_> {
    /// The human-identity refusal, then each path expanded with `expand_path` and held to
    /// `trusted_roots`, in the order given.
    ///
    /// # Errors
    ///
    /// Returns [`RemarginError::HumanIdentity`] when the server runs as a human and `tool` is not
    /// exempt, and a failure when a path cannot be expanded or lies outside the permitted roots.
    pub fn admit(&self, tool: &'static str, paths: &[&str]) -> Result<Vec<PathBuf>, RemarginError> {
        if matches!(self.config.author_type, Some(AuthorType::Human))
            && !HUMAN_IDENTITY_EXEMPT_TOOLS.contains(&tool)
        {
            return Err(RemarginError::HumanIdentity(HumanIdentityRejected::new(
                self.config.identity.as_deref().unwrap_or("<unknown>"),
                tool,
            )));
        }
        let mut expanded = Vec::with_capacity(paths.len());
        for raw in paths {
            let path = self.expand(raw)?;
            check_one_path(
                self.system,
                self.base_dir,
                &self.permissions,
                tool,
                &path.to_string_lossy(),
            )?;
            expanded.push(path);
        }
        Ok(expanded)
    }

    /// `raw` with `~` and `$VAR` expanded, held to nothing: for a path the tool reads without
    /// being confined to `trusted_roots`.
    ///
    /// # Errors
    ///
    /// Returns a failure when the path cannot be expanded.
    pub fn expand(&self, raw: &str) -> Result<PathBuf, RemarginError> {
        Ok(expand_path(self.system, raw).with_context(|| format!("expanding path {raw:?}"))?)
    }
}

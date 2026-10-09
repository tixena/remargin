//! `Remargin` - Enhanced inline review protocol and document access layer for markdown.
//!
//! This library provides functionality to parse, write, and manage inline review
//! comments in markdown documents. It supports comment threading, checksums,
//! signatures, and cross-document queries.

// The `plan` tool's MCP schema is one `serde_json::json!` invocation, too deep for the default
// limit.
#![recursion_limit = "256"]

pub mod activity;
pub mod advice;
pub mod comment_style;
pub mod config;
pub mod crypto;
pub mod display;
pub mod document;
pub mod frontmatter;
pub mod id;
pub mod kind;
pub mod linter;
pub mod mcp;
pub mod on_disk_comment;
pub mod operations;
pub mod parser;
pub mod path;
pub mod permissions;
pub mod reactions;
pub mod responses;
#[cfg(feature = "session")]
pub mod session;
pub mod writer;

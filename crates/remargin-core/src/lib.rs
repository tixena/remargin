//! `Remargin` - Enhanced inline review protocol and document access layer for markdown.
//!
//! This library provides functionality to parse, write, and manage inline review
//! comments in markdown documents. It supports comment threading, checksums,
//! signatures, and cross-document queries.

// The `plan` tool's MCP schema is built from a single `serde_json::json!`
// macro invocation that grew large enough to require a higher recursion
// limit (rem-0j2x added the `mv` op fields to it). 256 keeps headroom for
// further plan-op additions without the per-call cost of restructuring
// the macro into smaller pieces.
#![recursion_limit = "256"]

// Module declarations — uncommented as features are implemented.
pub mod activity;
pub mod config;
pub mod crypto;
pub mod display;
pub mod document;
pub mod frontmatter;
pub mod id;
pub mod kind;
pub mod linter;
pub mod mcp;
pub mod operations;
pub mod parser;
pub mod path;
pub mod permissions;
pub mod reactions;
pub mod skill;
pub mod writer;

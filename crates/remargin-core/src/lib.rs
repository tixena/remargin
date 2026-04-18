//! `Remargin` - Enhanced inline review protocol and document access layer for markdown.
//!
//! This library provides functionality to parse, write, and manage inline review
//! comments in markdown documents. It supports comment threading, checksums,
//! signatures, and cross-document queries.

// Module declarations — uncommented as features are implemented.
pub mod config;
pub mod crypto;
pub mod display;
pub mod document;
pub mod frontmatter;
pub mod id;
pub mod linter;
pub mod mcp;
pub mod operations;
pub mod parser;
pub mod path;
pub mod skill;
pub mod writer;

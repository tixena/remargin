//! End-to-end tests for `remargin get` half-open line windows.
//!
//! A lone `--start` is a tail to EOF; a lone `--end` is a head from line 1.
//! Regression coverage for the option-collapsing match that silently
//! returned the whole document when only one bound was supplied.

#[cfg(test)]
#[path = "cli_get_line_window/tests.rs"]
mod tests;

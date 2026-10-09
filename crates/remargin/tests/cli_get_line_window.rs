//! End-to-end tests for `remargin get` half-open line windows.
//!
//! A lone `--start` is a tail to EOF; a lone `--end` is a head from line 1.

#[cfg(test)]
#[path = "cli_get_line_window/tests.rs"]
mod tests;

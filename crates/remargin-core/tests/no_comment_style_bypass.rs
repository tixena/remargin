//! Structural guard: the comment-body gate must stay unbypassable.
//!
//! The gate takes the body and the author type and nothing else: with no config, no
//! environment and no third parameter reaching it, there is nowhere for a flag, a config key or
//! an environment variable to attach. The edit gate is held to the same shape, and the edit
//! path has to call it.
//!
//! This test reads the source, not the behaviour, because the thing being asserted is
//! the absence of a seam, and absences do not show up in a run.

#[cfg(test)]
#[path = "no_comment_style_bypass/tests.rs"]
mod tests;

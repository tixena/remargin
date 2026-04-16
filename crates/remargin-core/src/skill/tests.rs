//! Tests for the skill installer.

use std::path::Path;

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::skill::{self, Agent, SkillStatus};

// ── helpers ───────────────────────────────────────────────────────────────────

fn project_system() -> MockSystem {
    MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap()
}

fn global_system() -> MockSystem {
    MockSystem::new()
        .with_env("HOME", "/home/testuser")
        .unwrap()
        .with_dir(Path::new("/home/testuser"))
        .unwrap()
}

// ── path resolution ───────────────────────────────────────────────────────────

#[test]
fn install_claude_project() {
    let system = project_system();
    let path = skill::install(&system, Agent::Claude, false).unwrap();
    assert_eq!(path.to_str().unwrap(), "/project/.claude/skills/remargin");

    let content = system
        .read_to_string(Path::new("/project/.claude/skills/remargin/SKILL.md"))
        .unwrap();
    assert!(content.contains("remargin"));
}

#[test]
fn install_claude_global() {
    let system = global_system();
    let path = skill::install(&system, Agent::Claude, true).unwrap();
    assert_eq!(
        path.to_str().unwrap(),
        "/home/testuser/.claude/skills/remargin"
    );

    let content = system
        .read_to_string(Path::new("/home/testuser/.claude/skills/remargin/SKILL.md"))
        .unwrap();
    assert!(content.contains("remargin"));
}

#[test]
fn install_gemini_project() {
    let system = project_system();
    let path = skill::install(&system, Agent::Gemini, false).unwrap();
    assert_eq!(path.to_str().unwrap(), "/project/.gemini/skills/remargin");

    let content = system
        .read_to_string(Path::new("/project/.gemini/skills/remargin/SKILL.md"))
        .unwrap();
    assert!(content.contains("remargin"));
}

#[test]
fn install_gemini_global() {
    let system = global_system();
    let path = skill::install(&system, Agent::Gemini, true).unwrap();
    assert_eq!(
        path.to_str().unwrap(),
        "/home/testuser/.gemini/skills/remargin"
    );

    let content = system
        .read_to_string(Path::new("/home/testuser/.gemini/skills/remargin/SKILL.md"))
        .unwrap();
    assert!(content.contains("remargin"));
}

#[test]
fn agent_paths_do_not_overlap() {
    let system = project_system();
    let claude_path = skill::install(&system, Agent::Claude, false).unwrap();
    let gemini_path = skill::install(&system, Agent::Gemini, false).unwrap();
    assert_ne!(claude_path, gemini_path);
}

// ── status checks ─────────────────────────────────────────────────────────────

#[test]
fn status_not_installed_claude() {
    let system = project_system();
    let status = skill::test_status(&system, Agent::Claude, false).unwrap();
    assert_eq!(status, SkillStatus::NotInstalled);
}

#[test]
fn status_not_installed_gemini() {
    let system = project_system();
    let status = skill::test_status(&system, Agent::Gemini, false).unwrap();
    assert_eq!(status, SkillStatus::NotInstalled);
}

#[test]
fn status_up_to_date_after_install_claude() {
    let system = project_system();
    skill::install(&system, Agent::Claude, false).unwrap();
    let status = skill::test_status(&system, Agent::Claude, false).unwrap();
    assert_eq!(status, SkillStatus::UpToDate);
}

#[test]
fn status_up_to_date_after_install_gemini() {
    let system = project_system();
    skill::install(&system, Agent::Gemini, false).unwrap();
    let status = skill::test_status(&system, Agent::Gemini, false).unwrap();
    assert_eq!(status, SkillStatus::UpToDate);
}

#[test]
fn status_outdated_claude() {
    let system = project_system();
    skill::install(&system, Agent::Claude, false).unwrap();

    system
        .write(
            Path::new("/project/.claude/skills/remargin/SKILL.md"),
            b"old content",
        )
        .unwrap();

    let status = skill::test_status(&system, Agent::Claude, false).unwrap();
    assert_eq!(status, SkillStatus::Outdated);
}

#[test]
fn status_outdated_gemini() {
    let system = project_system();
    skill::install(&system, Agent::Gemini, false).unwrap();

    system
        .write(
            Path::new("/project/.gemini/skills/remargin/SKILL.md"),
            b"old content",
        )
        .unwrap();

    let status = skill::test_status(&system, Agent::Gemini, false).unwrap();
    assert_eq!(status, SkillStatus::Outdated);
}

#[test]
fn installing_claude_does_not_affect_gemini_status() {
    let system = project_system();
    skill::install(&system, Agent::Claude, false).unwrap();

    let gemini_status = skill::test_status(&system, Agent::Gemini, false).unwrap();
    assert_eq!(gemini_status, SkillStatus::NotInstalled);
}

// ── uninstall ─────────────────────────────────────────────────────────────────

#[test]
fn uninstall_claude_removes_only_claude_dir() {
    let system = project_system();
    skill::install(&system, Agent::Claude, false).unwrap();
    skill::install(&system, Agent::Gemini, false).unwrap();

    skill::uninstall(&system, Agent::Claude, false).unwrap();

    assert!(
        !system
            .exists(Path::new("/project/.claude/skills/remargin"))
            .unwrap()
    );
    assert!(
        system
            .exists(Path::new("/project/.gemini/skills/remargin"))
            .unwrap()
    );
}

#[test]
fn uninstall_gemini_removes_only_gemini_dir() {
    let system = project_system();
    skill::install(&system, Agent::Claude, false).unwrap();
    skill::install(&system, Agent::Gemini, false).unwrap();

    skill::uninstall(&system, Agent::Gemini, false).unwrap();

    assert!(
        system
            .exists(Path::new("/project/.claude/skills/remargin"))
            .unwrap()
    );
    assert!(
        !system
            .exists(Path::new("/project/.gemini/skills/remargin"))
            .unwrap()
    );
}

#[test]
fn uninstall_not_installed_returns_error() {
    let system = project_system();
    assert!(skill::uninstall(&system, Agent::Claude, false).is_err());
    assert!(skill::uninstall(&system, Agent::Gemini, false).is_err());
}

// ── error handling ────────────────────────────────────────────────────────────

#[test]
fn global_install_fails_without_home_claude() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();
    let result = skill::install(&system, Agent::Claude, true);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("HOME"));
}

#[test]
fn global_install_fails_without_home_gemini() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();
    let result = skill::install(&system, Agent::Gemini, true);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("HOME"));
}

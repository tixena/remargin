//! Tests for the skill installer.

use std::path::Path;

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::skill::{self, SkillStatus};

// ---------------------------------------------------------------------------
// Test 1: Install project-local
// ---------------------------------------------------------------------------

#[test]
fn install_project() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let path = skill::install(&system, false).unwrap();
    assert_eq!(path.to_str().unwrap(), "/project/.claude/skills/remargin");

    // Verify the SKILL.md file was written.
    let content = system
        .read_to_string(Path::new("/project/.claude/skills/remargin/SKILL.md"))
        .unwrap();
    assert!(content.contains("remargin"));
}

// ---------------------------------------------------------------------------
// Test 2: Install global
// ---------------------------------------------------------------------------

#[test]
fn install_global() {
    let system = MockSystem::new()
        .with_env("HOME", "/home/testuser")
        .unwrap()
        .with_dir(Path::new("/home/testuser"))
        .unwrap();

    let path = skill::install(&system, true).unwrap();
    assert_eq!(
        path.to_str().unwrap(),
        "/home/testuser/.claude/skills/remargin"
    );

    let content = system
        .read_to_string(Path::new("/home/testuser/.claude/skills/remargin/SKILL.md"))
        .unwrap();
    assert!(content.contains("remargin"));
}

// ---------------------------------------------------------------------------
// Test 3: Uninstall
// ---------------------------------------------------------------------------

#[test]
fn uninstall_removes_dir() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    // Install first.
    skill::install(&system, false).unwrap();

    // Then uninstall.
    skill::uninstall(&system, false).unwrap();

    // Verify directory is gone.
    let exists = system
        .exists(Path::new("/project/.claude/skills/remargin"))
        .unwrap();
    assert!(!exists);
}

// ---------------------------------------------------------------------------
// Test 4: Test not installed
// ---------------------------------------------------------------------------

#[test]
fn test_not_installed() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    let status = skill::test_status(&system, false).unwrap();
    assert_eq!(status, SkillStatus::NotInstalled);
}

// ---------------------------------------------------------------------------
// Test 5: Test outdated
// ---------------------------------------------------------------------------

#[test]
fn test_outdated() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    // Install first.
    skill::install(&system, false).unwrap();

    // Overwrite with different content.
    system
        .write(
            Path::new("/project/.claude/skills/remargin/SKILL.md"),
            b"old content",
        )
        .unwrap();

    let status = skill::test_status(&system, false).unwrap();
    assert_eq!(status, SkillStatus::Outdated);
}

// ---------------------------------------------------------------------------
// Test 6: Test up to date
// ---------------------------------------------------------------------------

#[test]
fn test_up_to_date() {
    let system = MockSystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_dir(Path::new("/project"))
        .unwrap();

    skill::install(&system, false).unwrap();

    let status = skill::test_status(&system, false).unwrap();
    assert_eq!(status, SkillStatus::UpToDate);
}

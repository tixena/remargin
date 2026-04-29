//! Unit tests for [`crate::operations::mv`].

use core::sync::atomic::{AtomicBool, Ordering};
use std::env::VarError;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;
use os_shim::{FileMetadata, System, TempDirHandle, WalkEntry};

use crate::config::{Mode, ResolvedConfig};
use crate::operations::mv::{MvArgs, mv};
use crate::parser::AuthorType;

/// Wrapper that turns the first `rename` call into an EXDEV error so we
/// can exercise the cross-filesystem fallback against a `MockSystem`
/// without needing two real mounts. Subsequent operations delegate
/// straight through.
struct ExdevSystem<'sys> {
    fired: AtomicBool,
    inner: &'sys MockSystem,
}

impl<'sys> ExdevSystem<'sys> {
    fn new(inner: &'sys MockSystem) -> Self {
        Self {
            fired: AtomicBool::new(false),
            inner,
        }
    }
}

impl System for ExdevSystem<'_> {
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }

    fn copy(&self, from: &Path, to: &Path) -> io::Result<u64> {
        self.inner.copy(from, to)
    }

    fn create(&self, path: &Path) -> io::Result<Box<dyn Write + '_>> {
        self.inner.create(path)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }

    fn create_temp_dir(&self) -> io::Result<Box<dyn TempDirHandle>> {
        self.inner.create_temp_dir()
    }

    fn current_dir(&self) -> io::Result<PathBuf> {
        self.inner.current_dir()
    }

    fn current_exe(&self) -> io::Result<PathBuf> {
        self.inner.current_exe()
    }

    fn env_var(&self, key: &str) -> Result<String, VarError> {
        self.inner.env_var(key)
    }

    fn exists(&self, path: &Path) -> io::Result<bool> {
        self.inner.exists(path)
    }

    fn is_dir(&self, path: &Path) -> io::Result<bool> {
        self.inner.is_dir(path)
    }

    fn is_file(&self, path: &Path) -> io::Result<bool> {
        self.inner.is_file(path)
    }

    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.inner.metadata(path)
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn Read + '_>> {
        self.inner.open(path)
    }

    fn open_append(&self, path: &Path) -> io::Result<Box<dyn Write + '_>> {
        self.inner.open_append(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.inner.read_dir(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.inner.read_to_string(path)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_file(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        if !self.fired.swap(true, Ordering::SeqCst) {
            return Err(io::Error::from_raw_os_error(18));
        }
        self.inner.rename(from, to)
    }

    fn set_env_var(&self, key: &str, value: &str) {
        self.inner.set_env_var(key, value);
    }

    fn walk_dir(
        &self,
        path: &Path,
        follow_links: bool,
        hidden: bool,
    ) -> io::Result<Vec<WalkEntry>> {
        self.inner.walk_dir(path, follow_links, hidden)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write(path, contents)
    }
}

fn base() -> &'static Path {
    Path::new("/realm")
}

fn open_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        unrestricted: false,
    }
}

fn realm_with(file: &str, contents: &[u8]) -> MockSystem {
    MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join(file), contents)
        .unwrap()
}

#[test]
fn cross_filesystem_rename_falls_back_to_copy() {
    let inner = realm_with("a.md", b"cross-fs payload");
    let system = ExdevSystem::new(&inner);

    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(outcome.fallback_copy);
    assert_eq!(outcome.bytes_moved, 16);
    assert!(!inner.exists(&base().join("a.md")).unwrap());
    assert_eq!(
        inner.read_to_string(&base().join("b.md")).unwrap(),
        "cross-fs payload"
    );
}

#[test]
fn force_overwrites_destination() {
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join("a.md"), b"new")
        .unwrap()
        .with_file(base().join("b.md"), b"old")
        .unwrap();

    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md")).with_force(true);
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(outcome.overwritten);
    assert!(!system.exists(&base().join("a.md")).unwrap());
    assert_eq!(system.read_to_string(&base().join("b.md")).unwrap(), "new");
}

#[test]
fn idempotent_when_source_already_at_destination() {
    // src is missing, dst exists — pretend a previous mv already succeeded.
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join("b.md"), b"already moved")
        .unwrap();

    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(!outcome.noop_same_path);
    assert!(!outcome.overwritten);
    // 0 distinguishes the "already settled" branch from a real move.
    assert_eq!(outcome.bytes_moved, 0);
    assert_eq!(outcome.dst_absolute, base().join("b.md"));
}

#[test]
fn missing_source_and_destination_errors() {
    let system = MockSystem::new().with_dir(base()).unwrap();
    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let err = mv(&system, base(), &open_config(), &args).unwrap_err();
    assert!(format!("{err}").contains("source not found"));
}

#[test]
fn moves_across_directories() {
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join("notes"))
        .unwrap()
        .with_dir(base().join("archive"))
        .unwrap()
        .with_file(base().join("notes/foo.md"), b"x")
        .unwrap();

    let args = MvArgs::new(
        PathBuf::from("notes/foo.md"),
        PathBuf::from("archive/foo.md"),
    );
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert_eq!(outcome.bytes_moved, 1);
    assert!(!system.exists(&base().join("notes/foo.md")).unwrap());
    assert!(system.exists(&base().join("archive/foo.md")).unwrap());
}

#[test]
fn refuses_destination_directory() {
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join("a.md"), b"x")
        .unwrap()
        .with_dir(base().join("dst"))
        .unwrap();

    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("dst"));
    let err = mv(&system, base(), &open_config(), &args).unwrap_err();
    assert!(
        format!("{err}").contains("destination is a directory"),
        "got: {err}"
    );
}

#[test]
fn refuses_existing_destination_without_force() {
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join("a.md"), b"src")
        .unwrap()
        .with_file(base().join("b.md"), b"dst")
        .unwrap();

    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let err = mv(&system, base(), &open_config(), &args).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("destination exists"), "got: {msg}");
    // Both files survive untouched.
    assert_eq!(system.read_to_string(&base().join("a.md")).unwrap(), "src");
    assert_eq!(system.read_to_string(&base().join("b.md")).unwrap(), "dst");
}

#[test]
fn refuses_forbidden_source_basename() {
    // `.remargin.yaml` is on the forbidden-target list — moving it
    // would let an agent route around the config-file write
    // protection.
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join(".remargin.yaml"), b"identity: alice\n")
        .unwrap();

    let args = MvArgs::new(
        PathBuf::from(".remargin.yaml"),
        PathBuf::from("backup.yaml"),
    );
    let err = mv(&system, base(), &open_config(), &args).unwrap_err();
    assert!(
        format!("{err}").contains("refusing to modify"),
        "got: {err}"
    );
}

#[test]
fn refuses_path_escape_on_source() {
    let system = realm_with("a.md", b"x");
    let args = MvArgs::new(PathBuf::from("../escape.md"), PathBuf::from("b.md"));
    let err = mv(&system, base(), &open_config(), &args).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("path escapes"), "got: {msg}");
}

#[test]
fn refuses_source_directory() {
    let system = MockSystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join("a"))
        .unwrap();
    let args = MvArgs::new(PathBuf::from("a"), PathBuf::from("b"));
    let err = mv(&system, base(), &open_config(), &args).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("source not found") || msg.contains("source is a directory"),
        "got: {msg}"
    );
}

#[test]
fn renames_within_same_dir() {
    let system = realm_with("a.md", b"hello world");
    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert_eq!(outcome.bytes_moved, 11);
    assert!(!outcome.fallback_copy);
    assert!(!outcome.noop_same_path);
    assert!(!outcome.overwritten);
    assert_eq!(outcome.dst_absolute, base().join("b.md"));
    assert!(!system.exists(&base().join("a.md")).unwrap());
    assert!(system.exists(&base().join("b.md")).unwrap());
    assert_eq!(
        system.read_to_string(&base().join("b.md")).unwrap(),
        "hello world"
    );
}

#[test]
fn same_path_is_noop() {
    let system = realm_with("a.md", b"unchanged");
    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("a.md"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(outcome.noop_same_path);
    assert!(!outcome.overwritten);
    assert_eq!(outcome.bytes_moved, 9);
    // File still there with same bytes.
    assert_eq!(
        system.read_to_string(&base().join("a.md")).unwrap(),
        "unchanged"
    );
}

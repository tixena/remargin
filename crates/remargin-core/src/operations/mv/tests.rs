//! Unit tests for [`crate::operations::mv`].

use core::future::ready;
use core::result;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::env::VarError;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use futures_io::{AsyncRead, AsyncWrite};
use os_shim::bucket::{Condition, Guarantees, RevisionToken};
use os_shim::error::Result;
use os_shim::mock::MemorySystem;
use os_shim::{BoxFuture, FileMetadata, System, TempDirHandle, WalkEntry};

use crate::config::{Mode, ResolvedConfig};
use crate::operations::mv::{MvArgs, mv};
use crate::parser::AuthorType;
use crate::wait::Wait as _;

/// Wrapper that turns the first `rename` call into an EXDEV error so we
/// can exercise the cross-filesystem fallback against a `MemorySystem`
/// without needing two real mounts. Subsequent operations delegate
/// straight through.
struct ExdevSystem<'sys> {
    fired: AtomicBool,
    inner: &'sys MemorySystem,
}

impl<'sys> ExdevSystem<'sys> {
    fn new(inner: &'sys MemorySystem) -> Self {
        Self {
            fired: AtomicBool::new(false),
            inner,
        }
    }
}

impl System for ExdevSystem<'_> {
    fn canonicalize<'system>(
        &'system self,
        path: &'system Path,
    ) -> BoxFuture<'system, Result<PathBuf>> {
        self.inner.canonicalize(path)
    }

    fn copy<'system>(
        &'system self,
        from: &'system Path,
        to: &'system Path,
    ) -> BoxFuture<'system, Result<u64>> {
        self.inner.copy(from, to)
    }

    fn create<'system>(
        &'system self,
        path: &'system Path,
    ) -> BoxFuture<'system, Result<Box<dyn AsyncWrite + Send + Unpin + 'system>>> {
        self.inner.create(path)
    }

    fn create_dir_all<'system>(
        &'system self,
        path: &'system Path,
    ) -> BoxFuture<'system, Result<()>> {
        self.inner.create_dir_all(path)
    }

    fn create_temp_dir<'system>(
        &'system self,
    ) -> BoxFuture<'system, Result<Box<dyn TempDirHandle + 'system>>> {
        self.inner.create_temp_dir()
    }

    fn current_dir(&self) -> Result<PathBuf> {
        self.inner.current_dir()
    }

    fn current_exe(&self) -> Result<PathBuf> {
        self.inner.current_exe()
    }

    fn env_var(&self, key: &str) -> result::Result<String, VarError> {
        self.inner.env_var(key)
    }

    fn exists<'system>(&'system self, path: &'system Path) -> BoxFuture<'system, Result<bool>> {
        self.inner.exists(path)
    }

    fn guarantees(&self) -> Guarantees {
        self.inner.guarantees()
    }

    fn is_dir<'system>(&'system self, path: &'system Path) -> BoxFuture<'system, Result<bool>> {
        self.inner.is_dir(path)
    }

    fn is_file<'system>(&'system self, path: &'system Path) -> BoxFuture<'system, Result<bool>> {
        self.inner.is_file(path)
    }

    fn metadata<'system>(
        &'system self,
        path: &'system Path,
    ) -> BoxFuture<'system, Result<FileMetadata>> {
        self.inner.metadata(path)
    }

    fn now(&self) -> SystemTime {
        self.inner.now()
    }

    fn open<'system>(
        &'system self,
        path: &'system Path,
    ) -> BoxFuture<'system, Result<Box<dyn AsyncRead + Send + Unpin + 'system>>> {
        self.inner.open(path)
    }

    fn open_append<'system>(
        &'system self,
        path: &'system Path,
    ) -> BoxFuture<'system, Result<Box<dyn AsyncWrite + Send + Unpin + 'system>>> {
        self.inner.open_append(path)
    }

    fn read<'system>(
        &'system self,
        path: &'system Path,
    ) -> BoxFuture<'system, Result<(Vec<u8>, RevisionToken)>> {
        self.inner.read(path)
    }

    fn read_dir<'system>(
        &'system self,
        path: &'system Path,
    ) -> BoxFuture<'system, Result<Vec<PathBuf>>> {
        self.inner.read_dir(path)
    }

    fn read_to_string<'system>(
        &'system self,
        path: &'system Path,
    ) -> BoxFuture<'system, Result<String>> {
        self.inner.read_to_string(path)
    }

    fn remove_dir_all<'system>(
        &'system self,
        path: &'system Path,
    ) -> BoxFuture<'system, Result<()>> {
        self.inner.remove_dir_all(path)
    }

    fn remove_file<'system>(&'system self, path: &'system Path) -> BoxFuture<'system, Result<()>> {
        self.inner.remove_file(path)
    }

    fn remove_file_if<'system>(
        &'system self,
        path: &'system Path,
        token: &'system RevisionToken,
    ) -> BoxFuture<'system, Result<bool>> {
        self.inner.remove_file_if(path, token)
    }

    fn rename<'system>(
        &'system self,
        from: &'system Path,
        to: &'system Path,
    ) -> BoxFuture<'system, Result<()>> {
        if !self.fired.swap(true, Ordering::SeqCst) {
            return Box::pin(ready(Err(io::Error::from_raw_os_error(18).into())));
        }
        self.inner.rename(from, to)
    }

    fn set_env_var(&self, key: &str, value: &str) {
        self.inner.set_env_var(key, value);
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'_, Result<()>> {
        self.inner.sleep(duration)
    }

    fn walk_dir<'system>(
        &'system self,
        path: &'system Path,
        follow_links: bool,
        hidden: bool,
    ) -> BoxFuture<'system, Result<Vec<WalkEntry>>> {
        self.inner.walk_dir(path, follow_links, hidden)
    }

    fn write<'system>(
        &'system self,
        path: &'system Path,
        contents: &'system [u8],
        condition: Condition,
    ) -> BoxFuture<'system, Result<RevisionToken>> {
        self.inner.write(path, contents, condition)
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
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

fn realm_with(file: &str, contents: &[u8]) -> MemorySystem {
    MemorySystem::new()
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

    assert!(outcome.action.fallback_copy);
    assert_eq!(outcome.bytes_moved, 16);
    assert!(!inner.exists(&base().join("a.md")).wait().unwrap());
    assert_eq!(
        inner.read_to_string(&base().join("b.md")).wait().unwrap(),
        "cross-fs payload"
    );
}

#[test]
fn force_overwrites_destination() {
    let system = MemorySystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join("a.md"), b"new")
        .unwrap()
        .with_file(base().join("b.md"), b"old")
        .unwrap();

    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md")).with_force(true);
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(outcome.action.overwritten);
    assert!(!system.exists(&base().join("a.md")).wait().unwrap());
    assert_eq!(
        system.read_to_string(&base().join("b.md")).wait().unwrap(),
        "new"
    );
}

#[test]
fn idempotent_when_source_already_at_destination() {
    let system = MemorySystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join("b.md"), b"already moved")
        .unwrap();

    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(!outcome.topology.noop_same_path);
    assert!(!outcome.action.overwritten);
    // 0 distinguishes the "already settled" branch from a real move.
    assert_eq!(outcome.bytes_moved, 0);
    assert_eq!(outcome.dst_absolute, base().join("b.md"));
}

#[test]
fn missing_source_and_destination_errors() {
    let system = MemorySystem::new().with_dir(base()).unwrap();
    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let err = mv(&system, base(), &open_config(), &args).unwrap_err();
    assert!(format!("{err}").contains("source not found"));
}

#[test]
fn moves_across_directories() {
    let system = MemorySystem::new()
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
    assert!(!system.exists(&base().join("notes/foo.md")).wait().unwrap());
    assert!(
        system
            .exists(&base().join("archive/foo.md"))
            .wait()
            .unwrap()
    );
}

#[test]
fn refuses_destination_directory() {
    let system = MemorySystem::new()
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
    let system = MemorySystem::new()
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
    assert_eq!(
        system.read_to_string(&base().join("a.md")).wait().unwrap(),
        "src"
    );
    assert_eq!(
        system.read_to_string(&base().join("b.md")).wait().unwrap(),
        "dst"
    );
}

#[test]
fn refuses_forbidden_source_basename() {
    let system = MemorySystem::new()
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
fn renames_empty_directory() {
    let system = MemorySystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join("a"))
        .unwrap();
    let args = MvArgs::new(PathBuf::from("a"), PathBuf::from("b"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(outcome.topology.is_directory);
    assert_eq!(outcome.nested_files_moved, 0);
    assert_eq!(outcome.bytes_moved, 0);
    assert!(!system.exists(&base().join("a")).wait().unwrap());
    assert!(system.is_dir(&base().join("b")).wait().unwrap());
}

#[test]
fn renames_within_same_dir() {
    let system = realm_with("a.md", b"hello world");
    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("b.md"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert_eq!(outcome.bytes_moved, 11);
    assert!(!outcome.action.fallback_copy);
    assert!(!outcome.topology.noop_same_path);
    assert!(!outcome.action.overwritten);
    assert_eq!(outcome.dst_absolute, base().join("b.md"));
    assert!(!system.exists(&base().join("a.md")).wait().unwrap());
    assert!(system.exists(&base().join("b.md")).wait().unwrap());
    assert_eq!(
        system.read_to_string(&base().join("b.md")).wait().unwrap(),
        "hello world"
    );
}

#[test]
fn same_path_is_noop() {
    let system = realm_with("a.md", b"unchanged");
    let args = MvArgs::new(PathBuf::from("a.md"), PathBuf::from("a.md"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(outcome.topology.noop_same_path);
    assert!(!outcome.action.overwritten);
    assert_eq!(outcome.bytes_moved, 9);
    assert_eq!(
        system.read_to_string(&base().join("a.md")).wait().unwrap(),
        "unchanged"
    );
}

#[test]
fn renames_directory_with_nested_md_files() {
    let system = MemorySystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join("notes"))
        .unwrap()
        .with_file(base().join("notes/a.md"), b"first")
        .unwrap()
        .with_file(base().join("notes/b.md"), b"second-content")
        .unwrap();

    let args = MvArgs::new(PathBuf::from("notes"), PathBuf::from("archive"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(outcome.topology.is_directory);
    assert_eq!(outcome.nested_files_moved, 2);
    assert_eq!(outcome.bytes_moved, 5 + 14);
    assert!(!system.exists(&base().join("notes")).wait().unwrap());
    assert_eq!(
        system
            .read_to_string(&base().join("archive/a.md"))
            .wait()
            .unwrap(),
        "first"
    );
    assert_eq!(
        system
            .read_to_string(&base().join("archive/b.md"))
            .wait()
            .unwrap(),
        "second-content"
    );
}

#[test]
fn renames_directory_with_mixed_content_and_subdirectory() {
    let system = MemorySystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join("src"))
        .unwrap()
        .with_dir(base().join("src/sub"))
        .unwrap()
        .with_file(base().join("src/a.md"), b"x")
        .unwrap()
        .with_file(base().join("src/notes.txt"), b"plain")
        .unwrap()
        .with_file(base().join("src/sub/c.md"), b"deep")
        .unwrap();

    let args = MvArgs::new(PathBuf::from("src"), PathBuf::from("dst"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(outcome.topology.is_directory);
    assert_eq!(outcome.nested_files_moved, 3);
    assert!(!system.exists(&base().join("src")).wait().unwrap());
    assert_eq!(
        system
            .read_to_string(&base().join("dst/a.md"))
            .wait()
            .unwrap(),
        "x"
    );
    assert_eq!(
        system
            .read_to_string(&base().join("dst/notes.txt"))
            .wait()
            .unwrap(),
        "plain"
    );
    assert_eq!(
        system
            .read_to_string(&base().join("dst/sub/c.md"))
            .wait()
            .unwrap(),
        "deep"
    );
}

#[test]
fn directory_same_path_is_noop() {
    let system = MemorySystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join("notes"))
        .unwrap()
        .with_file(base().join("notes/a.md"), b"keep")
        .unwrap();

    let args = MvArgs::new(PathBuf::from("notes"), PathBuf::from("notes"));
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(outcome.topology.noop_same_path);
    assert!(outcome.topology.is_directory);
    assert!(system.exists(&base().join("notes/a.md")).wait().unwrap());
}

#[test]
fn directory_refuses_existing_destination_without_force() {
    let system = MemorySystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join("src"))
        .unwrap()
        .with_dir(base().join("dst"))
        .unwrap();

    let args = MvArgs::new(PathBuf::from("src"), PathBuf::from("dst"));
    let err = mv(&system, base(), &open_config(), &args).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("destination exists"), "got: {msg}");
    assert!(system.is_dir(&base().join("src")).wait().unwrap());
    assert!(system.is_dir(&base().join("dst")).wait().unwrap());
}

#[test]
fn directory_force_overwrites_existing_destination() {
    let system = MemorySystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join("src"))
        .unwrap()
        .with_dir(base().join("dst"))
        .unwrap()
        .with_file(base().join("src/a.md"), b"new")
        .unwrap()
        .with_file(base().join("dst/old.md"), b"will-be-erased")
        .unwrap();

    let args = MvArgs::new(PathBuf::from("src"), PathBuf::from("dst")).with_force(true);
    let outcome = mv(&system, base(), &open_config(), &args).unwrap();

    assert!(outcome.topology.is_directory);
    assert!(outcome.action.overwritten);
    assert!(!system.exists(&base().join("src")).wait().unwrap());
    assert_eq!(
        system
            .read_to_string(&base().join("dst/a.md"))
            .wait()
            .unwrap(),
        "new"
    );
    assert!(!system.exists(&base().join("dst/old.md")).wait().unwrap());
}

#[test]
fn directory_refuses_dot_prefixed_source() {
    let system = MemorySystem::new()
        .with_dir(base())
        .unwrap()
        .with_dir(base().join(".hidden"))
        .unwrap();

    let args = MvArgs::new(PathBuf::from(".hidden"), PathBuf::from("visible"));
    let err = mv(&system, base(), &open_config(), &args).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("not visible"), "got: {msg}");
}

#[test]
fn directory_refused_when_deny_ops_covers_source() {
    let yaml = "permissions:\n  deny_ops:\n    - path: notes\n      ops: [mv]\n";
    let system = MemorySystem::new()
        .with_dir(base())
        .unwrap()
        .with_file(base().join(".remargin.yaml"), yaml.as_bytes())
        .unwrap()
        .with_dir(base().join("notes"))
        .unwrap()
        .with_file(base().join("notes/a.md"), b"x")
        .unwrap();

    let args = MvArgs::new(PathBuf::from("notes"), PathBuf::from("archive"));
    let err = mv(&system, base(), &open_config(), &args).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("denied by `deny_ops`"),
        "expected deny_ops refusal, got: {chain}"
    );
    assert!(system.is_dir(&base().join("notes")).wait().unwrap());
    assert!(!system.exists(&base().join("archive")).wait().unwrap());
}

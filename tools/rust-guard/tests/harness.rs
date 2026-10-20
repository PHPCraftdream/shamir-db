//! Shared harness for the rust-guard self-tests (CI-only).
//!
//! Materialises a fixture tree into a unique throwaway git repository
//! under the OS temp dir, and runs the built `rust-guard` binary
//! against it via the global `--repo` flag (cwd is the temp repo too,
//! so cwd-relative config files such as `.rust-guard/exceptions.toml`
//! resolve to the fixture copies).
//!
//! Conservation-only helpers (two-commit repos, delta files) are plain
//! functions in `conservation_tests.rs` itself, so the other test
//! binaries never carry dead code and no `#[allow]` is needed anywhere.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// Git identity flags for the throwaway repos.
const GIT_IDENTITY: [&str; 4] = [
    "-c",
    "user.email=rust-guard@example.invalid",
    "-c",
    "user.name=rust-guard",
];

const ROOT_MANIFEST: &str = "[workspace]\nmembers = [\"crates/fix\"]\nresolver = \"2\"\n";
const PKG_MANIFEST: &str = "[package]\nname = \"fix\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// A throwaway git repository holding a copy of one fixture tree.
pub struct TempRepo {
    /// Absolute path of the temp repo (also passed as `--repo`).
    pub root: PathBuf,
}

impl TempRepo {
    /// One-commit repo from `fixtures/<fixture>` (copied verbatim when
    /// present), plus the standard workspace scaffold, all committed.
    pub fn with_fixture(fixture: &str) -> Self {
        let repo = Self::empty_repo();
        copy_fixture(&repo.root, fixture);
        write_scaffold(&repo.root);
        repo.git(&["init"]);
        repo.commit_all("base");
        repo
    }

    fn empty_repo() -> Self {
        let root = unique_root();
        fs::create_dir_all(&root).expect("create temp dir");
        Self { root }
    }

    /// Stages everything and commits. `--allow-empty` so a fixture pair
    /// with identical trees still yields a two-commit history (finding 9:
    /// the second `git commit` otherwise fails before the guard runs).
    pub(crate) fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "--allow-empty", "-m", message]);
    }

    /// Runs git in the repo; panics on failure.
    pub fn git(&self, args: &[&str]) -> String {
        let mut full: Vec<&str> = GIT_IDENTITY.to_vec();
        full.extend_from_slice(args);
        let out = Command::new("git")
            .args(&full)
            .current_dir(&self.root)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            text(&out.stderr)
        );
        text(&out.stdout)
    }

    /// Runs the built binary with `--repo <root>` and cwd = repo root.
    pub fn run_guard(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rust-guard"))
            .arg("--repo")
            .arg(&self.root)
            .args(args)
            .current_dir(&self.root)
            .output()
            .expect("run rust-guard")
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Asserts the run failed and printed `code` in its findings output.
pub fn assert_finding(out: &Output, code: &str) {
    let err = text(&out.stderr);
    assert!(
        !out.status.success(),
        "expected exit != 0, got success\n{err}"
    );
    assert!(
        !err.contains("rust-guard: error"),
        "infra error, not findings:\n{err}"
    );
    assert!(err.contains(code), "expected {code} in output:\n{err}");
}

/// Asserts the run succeeded with zero findings.
pub fn assert_clean(out: &Output) {
    let err = text(&out.stderr);
    assert!(out.status.success(), "expected exit 0:\n{err}");
    assert!(err.contains("0 finding"), "expected no findings:\n{err}");
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn unique_root() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("rust-guard-selftest-{}-{n}", std::process::id()))
}

/// Copies `fixtures/<fixture>` over the repo (no-op when absent) and
/// returns the copied repo-relative paths (`/` separators).
pub(crate) fn copy_fixture(dst: &Path, fixture: &str) -> Vec<String> {
    let src = fixtures_dir().join(fixture);
    let mut copied = Vec::new();
    if src.is_dir() {
        copy_tree(&src, dst, &mut copied);
    }
    copied
}

/// Small recursive copy (no `fs_extra` dependency). `copied` collects
/// the absolute path of every file written (callers compare two copies
/// of the SAME repo, so absolute paths are directly comparable).
fn copy_tree(src: &Path, dst: &Path, copied: &mut Vec<String>) {
    for entry in fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            fs::create_dir_all(&target).expect("create dir");
            copy_tree(&entry.path(), &target, copied);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
            copied.push(target.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Writes the minimal workspace scaffold the `modules` gate needs
/// (`cargo metadata --no-deps` must succeed in the temp repo). A fixture
/// that ships its OWN root/package manifest keeps it: the fixture copy
/// happens first and existing files are never overwritten (finding 7:
/// conservation fixtures carry custom `[workspace]` members).
pub(crate) fn write_scaffold(root: &Path) {
    // A fixture that ships its OWN root manifest owns its whole member
    // list (finding 4): injecting a `crates/fix` package it never
    // declared left the package outside `cargo metadata`'s workspace
    // members while its sources stayed in the accounting scope —
    // UNACCOUNTED_FILE for the injected lib.rs. If such a fixture wants
    // a `fix` package it must ship it.
    if root.join("Cargo.toml").exists() {
        return;
    }
    for (rel, contents) in [
        ("Cargo.toml", ROOT_MANIFEST),
        ("crates/fix/Cargo.toml", PKG_MANIFEST),
        // A package manifest without a source target makes `cargo
        // metadata` fail BEFORE the tool ever reaches the fixture's own
        // (often deliberately broken) hand-scanned manifests — the
        // scaffold therefore always provides `crates/fix/src/lib.rs`
        // unless the fixture ships its own.
        ("crates/fix/src/lib.rs", "pub fn existing() -> u32 { 1 }\n"),
    ] {
        let path = root.join(rel);
        if path.exists() {
            continue;
        }
        fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
        fs::write(path, contents).expect("write manifest");
    }
}

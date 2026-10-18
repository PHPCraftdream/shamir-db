//! Shared harness for the rust-guard self-tests (CI-only).
//!
//! Materialises a fixture tree into a unique throwaway git repository
//! under the OS temp dir, and runs the built `rust-guard` binary
//! against it via the global `--repo` flag (cwd is the temp repo too,
//! so cwd-relative config files such as `.rust-guard/exceptions.toml`
//! resolve to the fixture copies).

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
    /// SHA of the first commit (two-commit conservation repos).
    #[allow(dead_code)] // conservation_tests only; dead in the other test binaries
    pub base_sha: String,
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

    /// Two-commit repo: `fixtures/<fixture>/base` is committed first,
    /// then `fixtures/<fixture>/head` is copied over and committed.
    /// `base_sha` holds the first commit's SHA.
    #[allow(dead_code)] // conservation_tests only; dead in the other test binaries
    pub fn two_commit(fixture: &str) -> Self {
        let mut repo = Self::with_fixture(&format!("{fixture}/base"));
        repo.base_sha = repo.head_sha();
        copy_fixture(&repo.root, &format!("{fixture}/head"));
        repo.commit_all("head");
        repo
    }

    fn empty_repo() -> Self {
        let root = unique_root();
        fs::create_dir_all(&root).expect("create temp dir");
        Self {
            root,
            base_sha: String::new(),
        }
    }

    /// Stages everything and commits.
    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", message]);
    }

    /// Full SHA of HEAD.
    fn head_sha(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_owned()
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

    /// Writes `contents` to `rel` inside the repo.
    #[allow(dead_code)] // conservation_tests only; dead in the other test binaries
    pub fn write(&self, rel: &str, contents: &str) {
        let path = self.root.join(rel);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).expect("create dir");
        }
        fs::write(path, contents).expect("write file");
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

fn copy_fixture(dst: &Path, fixture: &str) {
    let src = fixtures_dir().join(fixture);
    if src.is_dir() {
        copy_tree(&src, dst);
    }
}

/// Small recursive copy (no `fs_extra` dependency).
fn copy_tree(src: &Path, dst: &Path) {
    for entry in fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            fs::create_dir_all(&target).expect("create dir");
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// Writes the minimal workspace scaffold the `modules`/`conservation`
/// gates need (`cargo metadata --no-deps` must succeed in the temp repo).
fn write_scaffold(root: &Path) {
    for (rel, contents) in [
        ("Cargo.toml", ROOT_MANIFEST),
        ("crates/fix/Cargo.toml", PKG_MANIFEST),
    ] {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
        fs::write(path, contents).expect("write manifest");
    }
}

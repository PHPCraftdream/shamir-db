//! Read-only access to repository files, either from the working tree or
//! from git objects at a fixed revision (`--rev`). All paths are
//! repo-relative with forward slashes.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Why a file read failed. `Missing` means the path does not exist at
/// this rev / in this tree (a normal resolution outcome); `Io` means the
/// path exists but could not be read — callers must report that instead
/// of silently skipping (a silent skip hides files from every gate).
#[derive(Debug)]
pub enum ReadError {
    Missing,
    Io(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Missing => write!(f, "file not found at this revision"),
            ReadError::Io(msg) => write!(f, "unreadable: {msg}"),
        }
    }
}

/// Filesystem view backed by git (`git ls-files`, `git show <rev>:<path>`)
/// or, when no revision is given, by the working tree itself.
pub struct GitFs {
    rev: Option<String>,
    root: PathBuf,
}

impl GitFs {
    /// Opens the repo containing `cwd` (the `--repo` flag, `.` by
    /// default); the resolved toplevel becomes the root.
    pub fn open(rev: Option<&str>, cwd: &Path) -> Result<Self, String> {
        let root = run_git(Some(cwd), &["rev-parse", "--show-toplevel"])?;
        Ok(Self {
            rev: rev.map(str::to_owned),
            root: PathBuf::from(root.trim_end_matches(['\r', '\n'])),
        })
    }

    /// Repo root as an absolute path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All tracked files, repo-relative, `/` separators, sorted.
    pub fn tracked_files(&self) -> Result<Vec<String>, String> {
        let out = run_git(Some(&self.root), &["ls-files", "-z"])?;
        let mut files: Vec<String> = out
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(normalize)
            .collect();
        files.sort();
        Ok(files)
    }

    /// Reads a repo-relative file; `None` if it does not exist at this rev.
    /// Exists-but-unreadable files are returned as `None` too — use
    /// [`read_checked`] when the distinction matters (gates must report
    /// IO failures, never silently skip them).
    pub fn read(&self, rel: &str) -> Option<String> {
        self.read_checked(rel).ok()
    }

    /// Reads a repo-relative file, distinguishing "not there" from
    /// "there but unreadable".
    pub fn read_checked(&self, rel: &str) -> Result<String, ReadError> {
        // Repo-relative paths always use `/`; `Path::join` handles forward
        // slashes on every platform (on Windows they map to `\`, on Unix
        // they stay as-is). Never rewrite separators by hand: doing so
        // produces literal backslash filenames on Linux.
        let full = self.root.join(rel);
        match &self.rev {
            Some(rev) => {
                let spec = format!("{rev}:{rel}");
                let out = Command::new("git")
                    .args(["show", &spec])
                    .current_dir(&self.root)
                    .output()
                    .map_err(|e| ReadError::Io(format!("git show failed: {e}")))?;
                if !out.status.success() {
                    // `git show` fails both for missing objects and IO
                    // trouble; treat non-existence (the common case) as
                    // Missing and surface everything else verbatim.
                    let err = String::from_utf8_lossy(&out.stderr);
                    if err.contains("does not exist") || err.contains("exists on disk") {
                        return Err(ReadError::Missing);
                    }
                    return Err(ReadError::Io(err.trim().to_owned()));
                }
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            }
            None => match std::fs::read_to_string(full) {
                Ok(text) => Ok(text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ReadError::Missing),
                Err(e) => Err(ReadError::Io(format!("{rel}: {e}"))),
            },
        }
    }

    /// All paths (any type) present in the tree at this revision, via
    /// `git ls-tree -r -z --name-only <rev>`. Errors when no revision is
    /// pinned — conservation always reads immutable objects.
    pub fn list_tree(&self) -> Result<Vec<String>, String> {
        let rev = self.rev.as_deref().ok_or("no revision pinned")?;
        let out = run_git(
            Some(&self.root),
            &["ls-tree", "-r", "-z", "--name-only", rev],
        )?;
        Ok(out
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(normalize)
            .collect())
    }

    /// Resolves `rev` to a full commit SHA; `None` when it does not name
    /// a commit (used for the explicit blocked-comparison check).
    pub fn commit_sha(&self, rev: &str) -> Option<String> {
        let out = run_git(
            Some(&self.root),
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{rev}^{{commit}}"),
            ],
        )
        .ok()?;
        let sha = out.trim().to_owned();
        if sha.is_empty() {
            None
        } else {
            Some(sha)
        }
    }

    /// Existence check without reading the body.
    pub fn exists(&self, rel: &str) -> bool {
        match &self.rev {
            Some(_) => self.read(rel).is_some(),
            None => self.root.join(rel).is_file(),
        }
    }
}

/// Normalises a path reported by git to forward slashes (Windows-safe).
fn normalize(p: &str) -> String {
    p.replace('\\', "/")
}

/// Runs `git` and returns stdout; errors carry stderr for diagnostics.
fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

//! Read-only access to repository files, either from the working tree or
//! from git objects at a fixed revision (`--rev`). All paths are
//! repo-relative with forward slashes.

use std::path::{Path, PathBuf};
use std::process::Command;

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
    pub fn read(&self, rel: &str) -> Option<String> {
        match &self.rev {
            Some(rev) => {
                let spec = format!("{rev}:{rel}");
                Command::new("git")
                    .args(["show", &spec])
                    .current_dir(&self.root)
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            }
            None => std::fs::read_to_string(self.root.join(rel.replace('/', "\\"))).ok(),
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
            None => self.root.join(rel.replace('/', "\\")).is_file(),
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

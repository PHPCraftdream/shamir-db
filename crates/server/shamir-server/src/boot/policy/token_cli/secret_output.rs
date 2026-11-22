use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub enum SecretOutput {
    Stdout,
    File(std::fs::File, bool),
}

/// A rejected secret output destination. Nothing is ever written when this
/// error is returned.
#[derive(Debug, thiserror::Error)]
pub(super) enum OutputPathError {
    /// The path resolves into the reserved `audit.log*` namespace of the
    /// data directory: the active log, any rotation archive, or any entry
    /// below a reserved-named component. ASCII case variants are reserved
    /// on every platform, conservatively covering case-insensitive mounts
    /// (Windows, macOS defaults, case-insensitive filesystems on Linux).
    #[error(
        "the secret output path resolves to the audit log, one of its \
             archives, or something below that namespace; nothing was written"
    )]
    ReservedAuditName,
    /// An existing ancestor of the destination could not be resolved; the
    /// OS error detail is preserved.
    #[error("cannot resolve the secret output path: {0}")]
    Unresolvable(#[source] std::io::Error),
}

/// The reserved prefix is matched ASCII case-insensitively on EVERY
/// platform: that can only over-refuse on a strictly case-sensitive
/// filesystem, never under-refuse on case-insensitive mounts. The `dir`
/// prefix compared against is the canonicalized data directory, so the
/// exact on-disk spelling anchors the check.
fn component_starts_with_reserved(name: &std::ffi::OsStr) -> bool {
    const PREFIX: &[u8] = b"audit.log";
    let bytes = name.as_encoded_bytes();
    bytes.len() >= PREFIX.len() && bytes[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
}

/// True when `path` lies under `dir` and the component of `path` directly
/// below `dir` begins with the reserved `audit.log` prefix.
fn reserved_under(path: &Path, dir: &Path) -> bool {
    let mut path_components = path.components();
    for dir_component in dir.components() {
        let same = match path_components.next() {
            Some(c) => c
                .as_os_str()
                .eq_ignore_ascii_case(dir_component.as_os_str()),
            None => return false,
        };
        if !same {
            return false;
        }
    }
    path_components
        .next()
        .is_some_and(|next| component_starts_with_reserved(next.as_os_str()))
}

/// The destination where `prepare`'s create_new will land: the canonicalized
/// nearest existing ancestor of `path` plus the lexical remainder, so the
/// FINAL entry is the ORIGINAL name (existing entries and final links are
/// never followed for creation) while `..` segments and symlinked or missing
/// directories above it are resolved. This is operator input checked under
/// the maintenance lock; canonicalization is best-effort resolution, not a
/// defense against a concurrent adversary outside the audited directories.
fn lexical_candidate(path: &Path) -> Result<PathBuf, OutputPathError> {
    let mut last_missing: Option<std::io::Error> = None;
    for ancestor in path.ancestors().skip(1) {
        match std::fs::canonicalize(ancestor) {
            Ok(canonical_ancestor) => {
                // The remainder is what follows the resolved ancestor.
                let skip = ancestor.components().count();
                let mut candidate = canonical_ancestor;
                for component in path.components().skip(skip) {
                    candidate.push(component);
                }
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => last_missing = Some(e),
            Err(e) => return Err(OutputPathError::Unresolvable(e)),
        }
    }
    Err(OutputPathError::Unresolvable(last_missing.unwrap_or_else(
        || {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no resolvable ancestor of the output path",
            )
        },
    )))
}

fn is_dangling_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

/// Rejects a secret output path that resolves into the reserved audit
/// namespace of `data_dir` (case-insensitive component compare on Windows)
/// and returns the validated destination for `prepare` and cleanup, so no
/// later link resolution can differ from the validated one. Checked, in
/// order: the lexical destination under its canonical parent (a reserved
/// final name is refused even when an existing link points out), the whole
/// resolved entry (a final link must not reach a reserved target under a
/// harmless name), and — for a DANGLING final link only — its target through
/// the same bounded single-hop logic. A missing `data_dir` has no namespace
/// to collide with; a bare or fully relative path is resolved against the
/// current directory; a path with no final file name falls through to the
/// ordinary exclusive-create error of `prepare`.
pub(super) fn validate_out_file(path: &Path, data_dir: &Path) -> Result<PathBuf, OutputPathError> {
    let canonical_dir = match std::fs::canonicalize(data_dir) {
        Ok(dir) => dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(path.to_path_buf()),
        Err(e) => return Err(OutputPathError::Unresolvable(e)),
    };
    let path = if path.is_relative() {
        std::env::current_dir()
            .map_err(OutputPathError::Unresolvable)?
            .join(path)
    } else {
        path.to_path_buf()
    };
    let destination = lexical_candidate(&path)?;
    if reserved_under(&destination, &canonical_dir) {
        return Err(OutputPathError::ReservedAuditName);
    }
    if let Ok(full) = std::fs::canonicalize(&path) {
        if reserved_under(&full, &canonical_dir) {
            return Err(OutputPathError::ReservedAuditName);
        }
        return Ok(destination);
    }
    if is_dangling_symlink(&path) {
        if let Ok(target) = std::fs::read_link(&path) {
            let anchored = if target.is_relative() {
                destination
                    .parent()
                    .map(|parent| parent.join(target))
                    .unwrap_or_else(|| destination.clone())
            } else {
                target
            };
            let target_candidate = lexical_candidate(&anchored)?;
            if reserved_under(&target_candidate, &canonical_dir) {
                return Err(OutputPathError::ReservedAuditName);
            }
            if let Ok(full_target) = std::fs::canonicalize(&anchored) {
                if reserved_under(&full_target, &canonical_dir) {
                    return Err(OutputPathError::ReservedAuditName);
                }
            }
        }
    }
    Ok(destination)
}

pub fn prepare(path: Option<&Path>, terminal: bool) -> anyhow::Result<SecretOutput> {
    if let Some(path) = path {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(path)
            .map_err(|_| anyhow::anyhow!("cannot exclusively create secret output file"))?;
        Ok(SecretOutput::File(file, cfg!(windows)))
    } else if terminal {
        Ok(SecretOutput::Stdout)
    } else {
        anyhow::bail!("secret output unavailable; specify --out-file")
    }
}

pub fn deliver(
    output: SecretOutput,
    secret: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> io::Result<()> {
    match output {
        SecretOutput::Stdout => {
            stdout.write_all(secret.as_bytes())?;
            stdout.write_all(b"\n")
        }
        SecretOutput::File(mut file, warn_windows) => {
            if warn_windows {
                stderr.write_all(b"warning: Windows file ACLs were not tightened\n")?;
            }
            file.write_all(secret.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()
        }
    }
}

pub fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}

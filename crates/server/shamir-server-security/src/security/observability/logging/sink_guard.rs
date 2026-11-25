//! Sink selection and audit-namespace refusal for the diagnostic log.
//!
//! [`validate_sink_path`] rejects a configured sink that would land in the
//! audit-log namespace of `data_dir` before anything is opened. [`SinkMode`]
//! says which sink [`super::init_for`] installs, and [`sink_mode_for`] /
//! [`boot_logging`] are the shared halves of the boot-time selection that both
//! server entries use — a third entry cannot drift past the refusal.

use std::env;
use std::fs;
use std::path::Path;

use crate::config::LoggingConfig;

use super::LogGuard;

/// Refusals from [`validate_sink_path`].
#[derive(Debug, thiserror::Error)]
pub enum LogSinkError {
    /// The diagnostic log would be written into the audit-log namespace.
    #[error("the diagnostic log file resolves into the audit log namespace of the data directory; nothing was opened")]
    ReservedAuditName,
}

/// Refuse a diagnostic log sink that resolves into the audit-log namespace of
/// `data_dir` — the active audit log or a `audit.log.<digits>` rotation
/// archive — before anything is opened.
///
/// A sink in a SUBDIRECTORY of `data_dir` is allowed: only a log file that
/// lands directly in `data_dir` can collide with the appender's own files.
/// When `data_dir` does not exist yet there is no namespace, so `Ok(())`;
/// likewise when the sink's own parent directory does not exist yet, the sink
/// cannot be created at that place and there is nothing to refuse.
pub fn validate_sink_path(log_file: &Path, data_dir: &Path) -> Result<(), LogSinkError> {
    // Out of scope: a symlink (or a data directory swapped between this check
    // and the open) that redirects an otherwise innocent path into the audit
    // namespace. This rejects a misconfigured sink before it is opened; it is
    // not a sandbox boundary.
    let Ok(canonical_data_dir) = fs::canonicalize(data_dir) else {
        // NotFound — no data directory yet, so no audit namespace to protect.
        // Any other stat failure leaves no directory to compare against.
        return Ok(());
    };
    let resolved = if log_file.is_absolute() {
        log_file.to_path_buf()
    } else {
        match env::current_dir() {
            Ok(cwd) => cwd.join(log_file),
            Err(_) => return Ok(()),
        }
    };
    // Canonicalize the sink's PARENT only. The configured name itself may not
    // exist yet, but its directory must — if it does not, the sink cannot be
    // created at that place at all, so there is no file to refuse. Walking up
    // to the nearest existing ancestor instead would compare the wrong
    // directory: `data_dir/logs/audit.log` with `logs/` not yet created would
    // resolve to `data_dir` and be refused, contradicting the rule below that
    // a sink in a SUBDIRECTORY of `data_dir` is allowed. (A relative path was
    // resolved against the cwd above, so `parent()` is absolute.)
    let Some(parent) = resolved.parent() else {
        return Ok(());
    };
    let Ok(resolved_parent) = fs::canonicalize(parent) else {
        // The sink cannot be created at this place at all → nothing to refuse.
        return Ok(());
    };
    if resolved_parent != canonical_data_dir {
        return Ok(());
    }
    match resolved.file_name() {
        Some(name) if super::audit_appender::is_reserved_audit_name(name) => {
            Err(LogSinkError::ReservedAuditName)
        }
        _ => Ok(()),
    }
}

/// Which sink the subscriber installer takes (the guarded `boot_logging` path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkMode {
    /// Honour `LoggingConfig::file` (the server path). Reachable only through
    /// [`boot_logging`], which refuses an audit-namespace sink before opening.
    Config,
    /// Stdout only, exactly as if `file == None`. An offline command gets no
    /// file sink ([`SinkMode::StdoutOnly`]) because it must not create any
    /// file before its preconditions and its lock hold.
    StdoutOnly,
}

/// Sink for a command: offline commands must not create a file before their
/// preconditions and their lock hold.
pub fn sink_mode_for(offline: bool) -> SinkMode {
    if offline {
        SinkMode::StdoutOnly
    } else {
        SinkMode::Config
    }
}

/// Validate the configured sink and install the subscriber: the ONE boot path
/// both server entries (foreground `main` and the Windows service) use, so a
/// third entry cannot drift past the audit-namespace refusal.
///
/// `data_dir` is the server data directory the audit namespace belongs to —
/// `LoggingConfig` does not carry it, so the caller passes it alongside.
/// [`SinkMode::StdoutOnly`] never opens, creates or stat-s a file, so it
/// validates nothing.
pub fn boot_logging(
    cfg: &LoggingConfig,
    data_dir: &Path,
    mode: SinkMode,
) -> Result<LogGuard, LogSinkError> {
    // Exhaustive: a new `SinkMode` variant must say whether it opens a file.
    let configured_file = match mode {
        SinkMode::Config => cfg.file.as_deref(),
        SinkMode::StdoutOnly => None,
    };
    if let Some(file) = configured_file {
        validate_sink_path(Path::new(file), data_dir)?;
    }
    Ok(super::init_for(cfg, mode))
}

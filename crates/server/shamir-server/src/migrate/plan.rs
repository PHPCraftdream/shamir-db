use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use shamir_sync::format_stamp::migration::{
    chain_for, plan_migration, read, resume_plan, FormatStep, ResumePlan, JOURNAL_FILE_NAME,
};
use shamir_sync::format_stamp::{
    inspect, inspect_gate, CorruptKind, EpochPolicy, FormatStamp, StampError, StampState,
    STAMP_FILE_NAME, STAMP_TMP_NAME,
};

/// Read-only status of the live directory before any format-migration action.
#[derive(Debug)]
pub(crate) struct Preflight {
    pub state: String,
    pub epoch: Option<u64>,
    pub stamp: Option<FormatStamp>,
    pub needs_migration: bool,
    pub adopted: bool,
    pub stampless_data: bool,
}

/// Inspect the stamp and directory entries without writing, failing closed on directory errors.
pub(crate) fn inspect_preflight(
    dir: &Path,
    policy: EpochPolicy,
    configured_log_name: Option<&OsStr>,
) -> Result<Preflight> {
    let has_data = checked_data_present(dir, configured_log_name)?;
    let state = inspect(dir);
    let gate = inspect_gate(dir, &policy);

    match state {
        StampState::Absent { leftover_tmp: true } => Err(StampError::Corrupt {
            dir: dir.to_path_buf(),
            kind: CorruptKind::TornTmp,
        }
        .into()),
        StampState::Absent {
            leftover_tmp: false,
        } => {
            if !has_data && policy.base < policy.current {
                return match gate {
                    Err(error) if !matches!(error, StampError::AbsentOlderBase { .. }) => {
                        Err(error.into())
                    }
                    _ => Ok(Preflight {
                        state: "empty directory (no format data)".to_owned(),
                        epoch: Some(policy.base),
                        stamp: None,
                        needs_migration: false,
                        adopted: false,
                        stampless_data: false,
                    }),
                };
            }
            if has_data && policy.base < policy.current {
                match gate {
                    Err(StampError::AbsentOlderBase { .. }) => Ok(Preflight {
                        state: "stampless data (base epoch)".to_owned(),
                        epoch: Some(policy.base),
                        stamp: None,
                        needs_migration: true,
                        adopted: false,
                        stampless_data: true,
                    }),
                    Err(error) => Err(error.into()),
                    Ok(_) => Err(anyhow::anyhow!(
                        "read-only stamp check unexpectedly accepted older stampless data"
                    )),
                }
            } else {
                if let Err(error) = gate {
                    if !matches!(error, StampError::AbsentOlderBase { .. }) {
                        return Err(error.into());
                    }
                }
                Ok(Preflight {
                    state: if has_data {
                        "stampless data (base equals current)".to_owned()
                    } else {
                        "stampless empty directory".to_owned()
                    },
                    epoch: Some(policy.base),
                    stamp: None,
                    needs_migration: false,
                    adopted: false,
                    stampless_data: false,
                })
            }
        }
        StampState::Valid(stamp) => {
            let epoch = stamp.format_epoch();
            match gate {
                Ok(_) if epoch == policy.current => Ok(Preflight {
                    state: if stamp.created_by().adopted == Some(true) {
                        "adopted current stamp".to_owned()
                    } else {
                        "current stamp".to_owned()
                    },
                    epoch: Some(epoch),
                    adopted: stamp.created_by().adopted == Some(true),
                    stamp: Some(stamp),
                    needs_migration: false,
                    stampless_data: false,
                }),
                Err(StampError::OlderEpoch { found, .. })
                    if found == epoch && epoch >= policy.base =>
                {
                    Ok(Preflight {
                        state: if stamp.created_by().adopted == Some(true) {
                            "adopted older stamp".to_owned()
                        } else {
                            "older stamp".to_owned()
                        },
                        epoch: Some(epoch),
                        adopted: stamp.created_by().adopted == Some(true),
                        stamp: Some(stamp),
                        needs_migration: true,
                        stampless_data: false,
                    })
                }
                Err(error) => Err(error.into()),
                Ok(_) => Err(anyhow::anyhow!(
                    "read-only stamp check returned an inconsistent outcome for epoch {epoch}"
                )),
            }
        }
        StampState::Corrupt { .. } | StampState::NewerLayout { .. } => match gate {
            Err(error) => Err(error.into()),
            Ok(_) => Err(anyhow::anyhow!(
                "read-only stamp check unexpectedly accepted an unusable stamp"
            )),
        },
    }
}

/// Print the stable plan, including chain, journal resume status, gate verdict and known refusals.
pub(crate) fn render_plan(
    dir: &Path,
    policy: EpochPolicy,
    modules: &[&dyn FormatStep],
    preflight: &Preflight,
    backup_verdict: &str,
    confirmed: bool,
    out: &mut dyn Write,
) -> Result<()> {
    writeln!(out, "format migration plan")?;
    match preflight.epoch {
        Some(epoch) => writeln!(out, "stamp: {} (epoch {})", preflight.state, epoch)?,
        None => writeln!(out, "stamp: {} (epoch unknown)", preflight.state)?,
    }

    let mut refusal = None;
    let mut chain: Vec<&dyn FormatStep> = Vec::new();
    if preflight.needs_migration {
        let from_epoch = preflight.epoch.unwrap_or(policy.base);
        match chain_for(EpochPolicy::new(policy.current, from_epoch), modules) {
            Ok(resolved) => chain = resolved,
            Err(error) => {
                writeln!(out, "chain: refused ({error})")?;
                refusal = Some(format!("format-migration chain: {error}"));
            }
        }
        if refusal.is_none() {
            writeln!(out, "chain: {} -> {}", from_epoch, policy.current)?;
            if chain.is_empty() {
                writeln!(out, "  (no modules)")?;
            }
            for (index, step) in chain.iter().enumerate() {
                let descriptor = step.descriptor();
                writeln!(
                    out,
                    "  [{}] {}: {} | touches: {} | estimate: {}",
                    index + 1,
                    descriptor.id,
                    descriptor.summary,
                    descriptor.touches.join(", "),
                    descriptor.work_estimate
                )?;
            }
            match plan_migration(dir, policy, modules) {
                Ok(lines) => {
                    for line in &lines {
                        writeln!(out, "  plan: {}", line.description)?;
                    }
                }
                Err(error) => refusal = Some(format!("format-migration plan: {error}")),
            }
        }
    } else {
        writeln!(out, "chain: none (nothing to do)")?;
    }

    let journal_status = if preflight.needs_migration {
        journal_status(dir, preflight, policy, &chain)
    } else {
        "not consulted (nothing to do)".to_owned()
    };
    writeln!(out, "journal: {journal_status}")?;
    if journal_status.starts_with("corrupt") || journal_status.starts_with("mismatch") {
        refusal = Some(format!("journal: {journal_status}"));
    }
    writeln!(
        out,
        "permanent refusals: newer epoch, newer layout, corrupt stamp, torn tmp"
    )?;
    writeln!(out, "lock probe: not attempted (plan mode)")?;
    writeln!(out, "backup gate: {backup_verdict}")?;
    writeln!(
        out,
        "integrity scope: wire_tables MessagePack decode only; not table/index/WAL health"
    )?;
    writeln!(out, "doctor recommendation: run stopped-server doctor after successful format migration; not an integrity gate")?;
    writeln!(out, "gate coverage exceptions: cache/ (derived), .shamir.lock (created after backup), format_migration.journal (may be created after backup); no other live data names are ignored")?;
    writeln!(
        out,
        "strict manifest top-level coverage may reject empty directories absent from the manifest"
    )?;

    let mut refusals = Vec::new();
    if let Some(reason) = refusal {
        refusals.push(reason);
    }
    if preflight.adopted && preflight.needs_migration && !confirmed {
        refusals.push("adopted stamp requires --yes (operator confirmation)".to_owned());
    } else if preflight.needs_migration && !confirmed {
        refusals.push("execution requires --yes".to_owned());
    }
    if preflight.needs_migration
        && !backup_verdict.starts_with("PASS:")
        && !backup_verdict.starts_with("OVERRIDE:")
    {
        refusals.push("backup gate is not satisfied; run `backup --to <dir>` first, then use --from-backup <dir> or explicitly --i-have-a-backup".to_owned());
    }
    if preflight.stampless_data {
        refusals.push(
            "stampless older-base data requires --yes and a passing backup gate before execution"
                .to_owned(),
        );
    }
    if refusals.is_empty() {
        writeln!(out, "remaining refusals: none")?;
    } else {
        writeln!(out, "remaining refusals:")?;
        for reason in refusals {
            writeln!(out, "  - {reason}")?;
        }
    }
    Ok(())
}

fn journal_status(
    dir: &Path,
    preflight: &Preflight,
    policy: EpochPolicy,
    chain: &[&dyn FormatStep],
) -> String {
    if !preflight.needs_migration {
        return "not consulted (nothing to do)".to_owned();
    }
    let path = dir.join(JOURNAL_FILE_NAME);
    let records = match read(dir) {
        Ok(records) => records,
        Err(error) => {
            return format!(
                "corrupt/mismatch at line {}: {}; format migration refuses",
                error.line, error.message
            );
        }
    };
    if records.is_empty() {
        return match fs::metadata(&path) {
            Ok(metadata) if metadata.len() == 0 => "empty journal; fresh cursor 0".to_owned(),
            Ok(_) => {
                "no complete records (torn trailing record ignored); fresh cursor 0".to_owned()
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                "absent; fresh cursor 0".to_owned()
            }
            Err(error) => format!("unreadable metadata: {error}"),
        };
    }
    let expected_modules: Vec<String> = chain
        .iter()
        .map(|step| step.descriptor().id.to_owned())
        .collect();
    match resume_plan(
        &records,
        &expected_modules,
        preflight.epoch.unwrap_or(policy.base),
        policy.current,
        env!("CARGO_PKG_VERSION"),
    ) {
        ResumePlan::Fresh => "fresh; cursor 0".to_owned(),
        ResumePlan::Resume { next_module_index } if next_module_index < chain.len() => format!(
            "resume cursor {}/{}; next {}",
            next_module_index,
            chain.len(),
            chain[next_module_index].descriptor().id
        ),
        ResumePlan::Resume { next_module_index } => format!(
            "resume cursor {next_module_index}/{}; all steps complete, integrity/stamp finalization remains",
            chain.len()
        ),
        ResumePlan::Mismatch => "mismatch (chain, epochs, or binary version differ); format migration refuses".to_owned(),
    }
}

fn checked_data_present(dir: &Path, configured_log_name: Option<&OsStr>) -> Result<bool> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("read data directory {}", dir.display()))
        }
    };
    let mut has_data = false;
    for entry in entries {
        let entry =
            entry.with_context(|| format!("read entry in data directory {}", dir.display()))?;
        let name = entry.file_name();
        if name == OsStr::new(".shamir.lock")
            || name == OsStr::new(STAMP_FILE_NAME)
            || name == OsStr::new(STAMP_TMP_NAME)
            || configured_log_name == Some(name.as_os_str())
        {
            continue;
        }
        has_data = true;
    }
    Ok(has_data)
}

/// Return a configured log basename only when its normalized path is a direct child of data_dir.
pub(crate) fn configured_log_name(
    data_dir: &Path,
    log_file: Option<&str>,
) -> Result<Option<OsString>> {
    let Some(log_file) = log_file else {
        return Ok(None);
    };
    let cwd = std::env::current_dir().context("read current directory for log path")?;
    let data_absolute = lexical_absolute(&cwd, data_dir);
    let log_absolute = lexical_absolute(&cwd, Path::new(log_file));
    if log_absolute.parent() == Some(data_absolute.as_path()) {
        Ok(log_absolute.file_name().map(|name| name.to_os_string()))
    } else {
        Ok(None)
    }
}

fn lexical_absolute(cwd: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

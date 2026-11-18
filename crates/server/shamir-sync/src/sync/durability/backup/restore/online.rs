//! Kind-aware restore of an online copy: staged replay + consistency
//! (design `online-backup.md` §5). Child module of `restore.rs`.
//!
//! Orchestration for the ONLINE path only — the offline/v1 path stays
//! byte-for-byte in `restore.rs`. Runs after the snapshot has been copied
//! into the staged temp sibling and BEFORE ticket invalidation, marker and
//! swap, so every refusal leaves `data_dir` untouched and the caller
//! cleans the staged directory ([`super::cleanup_staged_temp_dir`]).
//!
//! Threading (brief 47): the apply/replay chain nests deep async frames,
//! so each phase runs on a dedicated thread with a roomy stack through
//! [`run_phase`], each with its OWN Tokio runtime. The phases are joined
//! sequentially:
//!
//! * Phase A (catalogue + replay): the runtime is fully dropped before
//!   phase B spawns, draining the staged system catalogue's background
//!   Tokio tasks so its Fjall meta lock is released before phase B opens
//!   the same staged meta store — the caller's runtime is never re-entered.
//! * Phase B (boot + read-only consistency pass): its runtime drop drains
//!   the booted engine's tasks so every staged handle is closed before the
//!   swap renames the tree.
//!
//! A replay refusal or failure aborts with a typed [`RestoreError`]
//! naming the repository and the reason ([`RestoreError::Replay`] /
//! [`RestoreError::Consistency`]); preflight failures (missing captured
//! directories, fake store directories) run before any normal boot could
//! create or initialize them.

use std::path::Path;

use shamir_db::engine::db_instance::replay_to::replay_to;

use super::catalogue::{self, CapturedRepo};
use super::fjall_locks::recreate_staged_fjall_locks;
use super::RestoreError;
use crate::sync::durability::backup::online::manifest::{ManifestV2, WalDirEntry};

/// Roomy stack for the dedicated phase threads (deep engine async frames;
/// see the brief's "on_big_stack" pattern).
const PHASE_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Run the full online pipeline on the staged copy. Returns the excluded
/// repositories (`db/repo`) for the report.
pub(super) fn run(stage: &Path, manifest: &ManifestV2) -> Result<Vec<String>, RestoreError> {
    // Lock artifacts first: nothing below opens a staged Fjall store
    // before this, so no live holder can race the create_new.
    recreate_staged_fjall_locks(stage)?;
    let captured = captured_repos(manifest)?;
    // Phase A: staged catalogue + strict cold replay of every captured
    // repository to its recorded point V*.
    let mut prepared = run_phase("restore-online-catalogue", {
        let stage = stage.to_path_buf();
        move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| phase_io("phase A runtime", e))?;
            let result = runtime.block_on(Box::pin(async {
                // Brief 47 / R2a 3b-2: before the stage can ever be
                // published, decode the staged wire-tables registry with a
                // private shape mirror of the boot decoder — a file the
                // server would fatally reject at boot refuses typed here,
                // while `data_dir` is still untouched.
                super::wire_registry::validate_staged_wire_tables(&stage).await?;
                let mut prepared = catalogue::prepare_staged_catalogue(&stage, &captured).await?;
                for job in std::mem::take(&mut prepared.replay_jobs) {
                    let report = Box::pin(replay_to(
                        &job.store_dir,
                        &job.wal_dir,
                        job.v_star,
                        job.scope,
                    ))
                    .await
                    .map_err(|source| RestoreError::Replay {
                        repo: job.label.clone(),
                        source: Box::new(source),
                    })?;
                    tracing::info!(
                        repo = %job.label,
                        v_star = job.v_star,
                        entries_applied = report.entries_applied,
                        "restore: staged repository replayed to its captured point"
                    );
                }
                Ok(prepared)
            }));
            drop(runtime); // drains phase A tasks, releases the meta lock
            result
        }
    })?;
    // Phase B: boot + read-only consistency pass; joined only after phase
    // A's thread (and runtime) has fully terminated.
    let excluded = std::mem::take(&mut prepared.excluded);
    run_phase("restore-online-consistency", {
        let stage = stage.to_path_buf();
        move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| phase_io("phase B runtime", e))?;
            let result = runtime.block_on(Box::pin(async {
                catalogue::boot_and_check_staged(&stage, &prepared).await
            }));
            drop(runtime); // drains phase B tasks, closes every staged handle
            result
        }
    })?;
    Ok(excluded.into_iter().collect())
}

/// Parse the manifest's captured set: `consistency.per_repo` keyed with
/// the `wal.dirs` entries. Duplicate WAL entries for one repo refuse.
fn captured_repos(manifest: &ManifestV2) -> Result<Vec<CapturedRepo>, RestoreError> {
    let consistency = manifest
        .consistency
        .as_ref()
        .ok_or_else(|| RestoreError::Consistency {
            repo: "(manifest)".into(),
            reason: "online manifest has no consistency section".into(),
        })?;
    // Mandatory WAL route: an online manifest without a wal section, or a
    // captured repository without its wal directory entry, refuses typed —
    // no sibling-name fallback is derived.
    let wal_section = manifest
        .wal
        .as_ref()
        .ok_or_else(|| RestoreError::Consistency {
            repo: "(manifest)".into(),
            reason: "online manifest has no wal section".into(),
        })?;
    let wal_dirs = wal_section
        .dirs
        .iter()
        .map(|dir| (dir.repo.as_str(), dir))
        .collect::<Vec<(&str, &WalDirEntry)>>();
    let mut captured = Vec::with_capacity(consistency.per_repo.len());
    for (key, point) in &consistency.per_repo {
        let (db, repo) = CapturedRepo::key(key)?;
        let mut wal_entries = wal_dirs.iter().filter(|(name, _)| *name == key.as_str());
        let wal = match wal_entries.next() {
            Some((_, dir)) => {
                if wal_entries.next().is_some() {
                    return Err(RestoreError::Consistency {
                        repo: key.clone(),
                        reason: "manifest lists more than one wal directory for this \
                                 repository"
                            .into(),
                    });
                }
                (dir.dir.clone(), dir.segments.len())
            }
            None => {
                return Err(RestoreError::Consistency {
                    repo: key.clone(),
                    reason: "online manifest lists no wal directory for this \
                             repository"
                        .into(),
                })
            }
        };
        captured.push(CapturedRepo {
            db,
            repo,
            store_dir: point.store_dir.clone(),
            wal,
            v_star: point.wal_position,
        });
    }
    Ok(captured)
}

/// Run one phase on a dedicated roomy thread; join it before returning so
/// ALL of the phase's work is done before any cleanup or swap runs.
fn run_phase<T: Send + 'static>(
    label: &'static str,
    body: impl FnOnce() -> Result<T, RestoreError> + Send + 'static,
) -> Result<T, RestoreError> {
    let handle = std::thread::Builder::new()
        .name(label.to_string())
        .stack_size(PHASE_STACK_SIZE)
        .spawn(body)
        .map_err(|e| phase_io("phase thread spawn", e))?;
    match handle.join() {
        Ok(result) => result,
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            Err(phase_io(
                "phase thread",
                std::io::Error::other(format!("panicked: {message}")),
            ))
        }
    }
}

fn phase_io(operation: &str, e: std::io::Error) -> RestoreError {
    RestoreError::Io(std::io::Error::new(
        e.kind(),
        format!("restore online phase {operation}: {e}"),
    ))
}

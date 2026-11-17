//! Orchestrator for the offline `logical-import` CLI:
//! lock → target check → preflight → idempotent-retry check → stage → load →
//! validate → receipt → publish, with progress on stderr. Cancellation is
//! observed at phase boundaries and between artifact frames and discards the
//! stage; after a successful publish it reports success.

use std::collections::BTreeMap;
use std::path::Path;

use sha2::Digest;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_server_security::user_directory::FjallUserDirectory;
use shamir_sync::format_stamp::{inspect, StampState};
use std::io::Read;
use tokio_util::sync::CancellationToken;

use super::args::LogicalImportArgs;
use super::error::LogicalImportError;
use super::load::preflight::Preflight;
use super::load::{stage_load, validate};
use super::reader::CancelledRead;
use super::stage::dir::{prepare_target_name, ImportStage};
use super::stage::lock::ImportLock;
use super::stage::receipt::{Receipt, RECEIPT_NAME};

/// Summary of a completed (or idempotently accepted) import.
#[derive(Clone, Debug)]
pub struct ImportReport {
    /// Effective imported database name (after `--as-db`).
    pub database: String,
    /// Portable items observed in the artifact.
    pub items: u64,
    /// Artifact size in bytes.
    pub bytes: u64,
    /// False for an idempotent retry of an already-published target.
    pub published: bool,
}

/// Offline import. Not cancel-safe as a future: the caller must retain and
/// join this future (the CLI does), signalling cancellation with the token.
/// The lock is held for the whole run and removed only after the stage
/// discard/publish outcome is resolved (RAII drop at function exit).
pub async fn import(
    args: LogicalImportArgs,
    cancel: CancellationToken,
) -> Result<ImportReport, LogicalImportError> {
    // Exclusive ownership BEFORE the target is inspected.
    let lock = ImportLock::acquire(&args.target)?;
    progress("lock acquired");
    if let Some(report) = retry_existing(&args, &cancel).await? {
        drop(lock);
        return Ok(report);
    }
    prepare_target_name(&args.target)?;
    let preflight = Preflight::run(&args.input, &args, &cancel)?;
    progress(&format!(
        "preflight verified: {} identities, {} repositories, {} tables",
        preflight.identities.len(),
        preflight.repos.len(),
        preflight.tables.len()
    ));
    let stage = ImportStage::prepare(&args.target)?;
    let outcome = import_into_stage(&stage, &args, &preflight, &cancel).await;
    match outcome {
        Ok(report) => match stage.publish(&args.target) {
            Ok(()) => {
                drop(lock);
                progress(&format!("published {}", args.target.display()));
                Ok(report)
            }
            Err(e) => Err(e),
        },
        Err(e) => {
            stage.discard()?;
            Err(e)
        }
    }
}

async fn import_into_stage(
    stage: &ImportStage,
    args: &LogicalImportArgs,
    preflight: &Preflight,
    cancel: &CancellationToken,
) -> Result<ImportReport, LogicalImportError> {
    let loaded = stage_load::run(stage.path(), &args.input, preflight, args, cancel).await?;
    progress(&format!(
        "loaded into stage as database '{}'",
        loaded.db_name
    ));
    validate::run(
        &loaded.home,
        preflight,
        &loaded.db_name,
        &loaded.owner_ids,
        &args.input,
        cancel,
    )
    .await?;
    progress("validated against the artifact");
    let epoch = match inspect(stage.path()) {
        StampState::Valid(stamp) => stamp.format_epoch(),
        _ => {
            return Err(LogicalImportError::Stage(
                "staged format stamp is unreadable after load".into(),
            ))
        }
    };
    let receipt = Receipt {
        artifact_sha256: artifact_sha256(&args.input, cancel)?,
        source_db: preflight.database.name.clone(),
        imported_db: loaded.db_name.clone(),
        schema_only: args.schema_only,
        format_epoch: epoch,
        generator: format!(
            "shamir-server logical-import v{}",
            env!("CARGO_PKG_VERSION")
        ),
    };
    receipt.write(stage.path())?;
    if cancel.is_cancelled() {
        return Err(LogicalImportError::Cancelled);
    }
    // Handle-lifecycle invariant: every handle the import opened (the staged
    // engine, its repositories and the user directory) is closed and dropped
    // BEFORE the publish rename, exactly once per instance — no database may
    // survive a publish or the next open of the directory fails on the lock.
    drop(loaded.home);
    Ok(ImportReport {
        database: loaded.db_name,
        items: preflight.report.items,
        bytes: preflight.report.bytes,
        published: true,
    })
}

/// A complete existing target is accepted ONLY when its receipt matches the
/// artifact digest and effective options AND an integrity check passes
/// (readable format stamp, engine re-opens, content still equals the
/// artifact). Anything else is a typed refusal — never an overwrite.
async fn retry_existing(
    args: &LogicalImportArgs,
    cancel: &CancellationToken,
) -> Result<Option<ImportReport>, LogicalImportError> {
    let meta = match std::fs::symlink_metadata(&args.target) {
        Ok(meta) => meta,
        Err(_) => return Ok(None),
    };
    if !meta.is_dir() {
        return Err(LogicalImportError::Target(format!(
            "target {} exists and is not a directory",
            args.target.display()
        )));
    }
    let preflight = Preflight::run(&args.input, args, cancel)?;
    let receipt = match Receipt::read(&args.target)? {
        Some(receipt) => receipt,
        None => {
            return Err(LogicalImportError::Receipt(format!(
                "target {} exists without a {RECEIPT_NAME} receipt",
                args.target.display()
            )))
        }
    };
    let db_name = args
        .as_db
        .clone()
        .unwrap_or_else(|| preflight.database.name.clone());
    if !receipt.matches(
        &artifact_sha256(&args.input, cancel)?,
        &preflight.database.name,
        &db_name,
        args.schema_only,
    ) {
        return Err(LogicalImportError::Receipt(format!(
            "target {} was imported from a different artifact or options",
            args.target.display()
        )));
    }
    progress("complete target with matching receipt; re-validating");
    match inspect(&args.target) {
        StampState::Valid(_) => {}
        _ => {
            return Err(LogicalImportError::Receipt(format!(
                "target {} does not carry a readable format stamp",
                args.target.display()
            )))
        }
    }
    let owner_ids = target_owner_ids(&args.target, &preflight)?;
    let home = ShamirDb::init(SystemStoreConfig::Fjall(
        args.target.join("shamir_db_meta.redb"),
    ))
    .await
    .map_err(|e| LogicalImportError::Receipt(format!("cannot re-open the target engine: {e}")))?;
    validate::run(&home, &preflight, &db_name, &owner_ids, &args.input, cancel)
        .await
        .map_err(|e| {
            LogicalImportError::Receipt(format!("target no longer matches the artifact: {e}"))
        })?;
    Ok(Some(ImportReport {
        database: db_name,
        items: preflight.report.items,
        bytes: preflight.report.bytes,
        published: false,
    }))
}

/// The retry re-derives the owner-name → principal64 map from the target's
/// user directory (`list_all` includes disabled imported identities; the
/// `lookup_by_name` auth path stays closed by design).
fn target_owner_ids(
    target: &Path,
    preflight: &Preflight,
) -> Result<BTreeMap<String, u64>, LogicalImportError> {
    let directory = FjallUserDirectory::open(target.join("users"))
        .map_err(|e| LogicalImportError::Receipt(format!("cannot read target users: {e}")))?;
    let listed = directory
        .list_all()
        .map_err(|e| LogicalImportError::Receipt(format!("cannot list target users: {e}")))?;
    let mut by_name: BTreeMap<&str, u64> = BTreeMap::new();
    for (id, state) in &listed {
        by_name.insert(state.username.as_str(), *id);
    }
    let mut owners = BTreeMap::new();
    for name in preflight.identities.keys() {
        let id = by_name.get(name.as_str()).copied().ok_or_else(|| {
            LogicalImportError::Receipt(format!("target user directory lost identity '{name}'"))
        })?;
        owners.insert(name.clone(), id);
    }
    Ok(owners)
}

/// Lowercase hex SHA-256 over the whole artifact file (streamed, bounded by
/// the cancellation reader).
fn artifact_sha256(input: &Path, cancel: &CancellationToken) -> Result<String, LogicalImportError> {
    let file = std::fs::File::open(input)?;
    let mut hasher = sha2::Sha256::new();
    let mut reader = CancelledRead::new(file, cancel.clone());
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn progress(message: &str) {
    eprintln!("logical-import: {message}");
}

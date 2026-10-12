use std::fs;
use std::path::{Path, PathBuf};

use shamir_db::engine::repo::{
    BoxRepoFactory, RepoFactory, RepoInstance, SeedInstallState, SeedReceipt,
};
use shamir_db::storage::types::Repo;

use crate::replication_seed::artifact_writer::{sync_directory, write_new};
use crate::replication_seed::codec::digest;
use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

pub(super) fn destination(root: &Path, db: &str, repo: &str) -> PathBuf {
    let key = digest(format!("{}:{db}{repo}", db.len()).as_bytes());
    root.join(format!("seed-{key}"))
}

pub(super) fn fresh(target: &Path) -> Result<PathBuf, SeedError> {
    let parent = target
        .parent()
        .ok_or_else(|| invalid("seed parent missing"))?;
    let prefix = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("seed name is not UTF-8"))?;
    for attempt in 0..u64::MAX {
        let path = parent.join(format!("{prefix}-attempt-{attempt}"));
        match fs::create_dir(&path) {
            Ok(()) => {
                // Designation survives even interruption BEFORE Installing.
                write_new(&path.join(".seed-install"), b"SEED-3")?;
                sync_directory(&path)?;
                sync_directory(parent)?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(invalid("seed attempt namespace exhausted"))
}

pub(super) async fn completed(
    path: &Path,
    receipt: &SeedReceipt,
    published: bool,
) -> Result<bool, SeedError> {
    if !path.join(".seed-install").is_file() {
        return if published {
            Err(invalid("existing ordinary repository"))
        } else {
            // A crash creating the designation itself is an abandoned attempt.
            Ok(false)
        };
    }
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(invalid("seed symlink refused"));
    }
    let backing = BoxRepoFactory::fjall_raw(path).create().await?;
    let info = backing.store_get("__tx__").await?;
    match SeedInstallState::load(&info).await? {
        Some(SeedInstallState::Complete(_)) => {
            Ok(SeedInstallState::completed_retry(&info, receipt).await?)
        }
        Some(SeedInstallState::Installing(actual)) if actual != *receipt => {
            Err(invalid("conflicting incomplete seed receipt"))
        }
        _ if published => Err(invalid("published seed is incomplete")),
        _ => Ok(false),
    }
}

pub(super) async fn find_complete(
    target: &Path,
    receipt: &SeedReceipt,
) -> Result<Option<PathBuf>, SeedError> {
    if target.exists() {
        completed(target, receipt, true).await?;
        return Ok(Some(target.to_owned()));
    }
    let prefix = format!(
        "{}-attempt-",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("invalid target name"))?,
    );
    let mut paths = Vec::new();
    for entry in fs::read_dir(target.parent().ok_or_else(|| invalid("missing parent"))?)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&prefix))
        {
            if paths.len() >= 4096 {
                return Err(invalid("seed attempt inventory budget exceeded"));
            }
            paths.push(entry.path());
        }
    }
    paths.sort();
    let mut complete = None;
    for path in paths {
        if completed(&path, receipt, false).await? {
            if complete.is_some() {
                return Err(invalid("multiple completed seed attempts"));
            }
            complete = Some(path);
        }
    }
    Ok(complete)
}

pub(super) async fn open_image(path: &Path, name: &str) -> Result<RepoInstance, SeedError> {
    let backing = BoxRepoFactory::fjall_raw(path).create().await?;
    Ok(RepoInstance::new(name.to_owned(), backing, Vec::new()))
}

pub(super) fn publish(stage: &Path, target: &Path) -> Result<(), SeedError> {
    sync_directory(stage)?;
    if stage != target {
        if target.exists() {
            return Err(invalid("seed destination appeared before rename"));
        }
        fs::rename(stage, target)?;
    }
    sync_directory(target.parent().ok_or_else(|| invalid("missing parent"))?)
}

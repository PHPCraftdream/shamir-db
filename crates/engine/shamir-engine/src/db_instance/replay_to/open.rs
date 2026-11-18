//! Offline store open and strict replay of preflighted entries.

use std::path::Path;

use shamir_wal::WalEntryV2;

use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;

use super::api::ReplayToError;

pub(super) async fn replay(
    name: String,
    store_dir: &Path,
    configs: Vec<TableConfig>,
    entries: Vec<WalEntryV2>,
    v_star: u64,
) -> Result<usize, ReplayToError> {
    let factory = offline_factory(store_dir)?;
    Box::pin(RepoInstance::replay_staged_wal(
        name, factory, configs, entries, v_star,
    ))
    .await
    .map_err(|(open_failed, error)| {
        if open_failed {
            ReplayToError::Open(error)
        } else {
            ReplayToError::Recovery(error)
        }
    })
}

#[cfg(feature = "fjall")]
fn offline_factory(store_dir: &Path) -> Result<BoxRepoFactory, ReplayToError> {
    Ok(BoxRepoFactory::fjall_raw(store_dir))
}

#[cfg(not(feature = "fjall"))]
fn offline_factory(_store_dir: &Path) -> Result<BoxRepoFactory, ReplayToError> {
    Err(ReplayToError::UnsupportedBackend)
}

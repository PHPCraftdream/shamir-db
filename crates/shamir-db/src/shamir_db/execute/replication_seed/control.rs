use shamir_engine::repo::SeedReceipt;
use shamir_query_types::admin::ReplDirection;
use shamir_query_types::wire::repl::{ReplRepoInfo, ReplResponse};

use crate::query::admin::CreateSubscriptionOp;
use crate::query::batch::BatchError;
use crate::types::value::QueryValue;
use crate::ShamirDb;

use super::catalogue::{binding, matches_scope, receipt, rows, streams};
use super::SeedRefusal;

pub(crate) fn batch_error(reason: SeedRefusal) -> BatchError {
    BatchError::QueryError {
        alias: String::new(),
        message: reason.to_string(),
        code: Some(reason.code().into()),
    }
}

/// Metadata-only fence; callers must invoke before decoding ANY event payload.
pub fn fence_metadata(
    receipt: &SeedReceipt,
    lineage: Option<[u8; 16]>,
    epoch: Option<u64>,
    schema: Option<&str>,
) -> Result<(), SeedRefusal> {
    let lineage = lineage.ok_or(SeedRefusal::MissingIdentity)?;
    let epoch = epoch.ok_or(SeedRefusal::MissingIdentity)?;
    if lineage != receipt.source_identity.lineage_id {
        return Err(SeedRefusal::WrongIdentity);
    }
    if epoch != receipt.source_identity.history_epoch {
        return Err(SeedRefusal::WrongHistoryEpoch);
    }
    if schema != Some(receipt.schema_digest.as_str()) {
        return Err(SeedRefusal::WrongSchema);
    }
    Ok(())
}

pub fn validate_seed_hello(
    receipt: &SeedReceipt,
    bookmark: u64,
    publication: &str,
    hello: &ReplResponse,
) -> Result<(), SeedRefusal> {
    bookmark
        .checked_add(1)
        .ok_or(SeedRefusal::BookmarkOverflow)?;
    let ReplResponse::Hello { repos, .. } = hello else {
        return Err(SeedRefusal::UpstreamUnavailable);
    };
    let repo = hello_repo(receipt, repos)?;
    fence_metadata(
        receipt,
        repo.repo_lineage_id,
        repo.history_epoch,
        repo.schema_digest.as_deref(),
    )?;
    let publication = repo
        .publications
        .iter()
        .find(|item| item.create_publication == publication)
        .ok_or(SeedRefusal::WrongPublication)?;
    if publication.scopes.len() != 1 {
        return Err(SeedRefusal::WrongScope);
    }
    let scope = &publication.scopes[0];
    if scope.db != receipt.source_db
        || scope.repo.as_deref() != Some(receipt.source_repo.as_str())
        || scope.table.is_some()
    {
        return Err(SeedRefusal::WrongScope);
    }
    if repo.gap_through.is_some_and(|through| through > bookmark)
        || repo.journal_floor
            > bookmark
                .checked_add(1)
                .ok_or(SeedRefusal::BookmarkOverflow)?
    {
        return Err(SeedRefusal::HorizonAboveBookmark);
    }
    if repo.settled_version.ok_or(SeedRefusal::SourceRegression)? < receipt.source_version {
        return Err(SeedRefusal::SourceRegression);
    }
    Ok(())
}

fn hello_repo<'a>(
    receipt: &SeedReceipt,
    repos: &'a [ReplRepoInfo],
) -> Result<&'a ReplRepoInfo, SeedRefusal> {
    repos
        .iter()
        .find(|repo| repo.db == receipt.source_db && repo.repo == receipt.source_repo)
        .ok_or(SeedRefusal::MissingIdentity)
}

pub(super) async fn check_owners(
    home: &ShamirDb,
    name: &str,
    receipt: &SeedReceipt,
) -> Result<(), SeedRefusal> {
    for row in rows(home, "subscriptions")
        .await
        .map_err(|_| SeedRefusal::CompetingOwner)?
    {
        if row["name"].as_str() == Some(name) {
            continue;
        }
        let profile = row["profile"].as_str().ok_or(SeedRefusal::WrongScope)?;
        for stream in streams(home, profile).await? {
            if stream.direction == ReplDirection::Pull
                && stream.scope.db == receipt.source_db
                && (stream.scope.repo.is_none()
                    || stream.scope.repo.as_deref() == Some(receipt.source_repo.as_str()))
            {
                return Err(SeedRefusal::CompetingOwner);
            }
        }
    }
    Ok(())
}

impl ShamirDb {
    pub(crate) async fn validate_seed_profile_change(
        &self,
        profile: &str,
        proposed: &[crate::query::admin::ReplStream],
    ) -> Result<(), SeedRefusal> {
        for row in rows(self, "subscriptions")
            .await
            .map_err(|_| SeedRefusal::WrongScope)?
        {
            if row["profile"].as_str() != Some(profile) {
                continue;
            }
            for stream in streams(self, profile).await? {
                let Some(name) = stream.scope.repo else {
                    continue;
                };
                let Some(repo) = self
                    .get_db(&stream.scope.db)
                    .and_then(|db| db.get_repo(&name))
                else {
                    continue;
                };
                if let Some(receipt) = receipt(&repo).await? {
                    if !matches_scope(proposed, &receipt) {
                        return Err(SeedRefusal::WrongScope);
                    }
                }
            }
        }
        Ok(())
    }
    pub(crate) async fn validate_seed_resume_action(
        &self,
        record: &QueryValue,
        action: &crate::query::admin::SubAction,
    ) -> Result<(), SeedRefusal> {
        match action {
            crate::query::admin::SubAction::Resume => self.validate_seed_resume(record).await,
            crate::query::admin::SubAction::Pause => Ok(()),
            crate::query::admin::SubAction::SetProfile(_) => {
                if record.get("seed_receipt").is_some() {
                    Err(SeedRefusal::WrongScope)
                } else {
                    Ok(())
                }
            }
        }
    }
    pub(crate) async fn bind_seed_creation(
        &self,
        op: &CreateSubscriptionOp,
        record: &mut QueryValue,
    ) -> Result<(), SeedRefusal> {
        // Unresolved legacy profiles retain their ordinary behaviour. Seed targets,
        // however, are always derived from repository metadata, never row evidence.
        let Ok(scope) = streams(self, &op.profile).await else {
            return Ok(());
        };
        for stream in &scope {
            let Some(database) = self.get_db(&stream.scope.db) else {
                continue;
            };
            for name in database.list_repos() {
                if stream.scope.repo.as_ref().is_some_and(|repo| repo != &name) {
                    continue;
                }
                let Some(repo) = database.get_repo(&name) else {
                    continue;
                };
                if let Some(receipt) = receipt(&repo).await? {
                    if !matches_scope(&scope, &receipt) {
                        return Err(SeedRefusal::WrongScope);
                    }
                    if !op.paused {
                        return Err(SeedRefusal::ConflictingBinding);
                    }
                    check_owners(self, &op.create_subscription, &receipt).await?;
                    binding(record, &receipt);
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn validate_seed_resume(
        &self,
        record: &QueryValue,
    ) -> Result<(), SeedRefusal> {
        let profile = record["profile"].as_str().ok_or(SeedRefusal::WrongScope)?;
        let scope = streams(self, profile).await?;
        if let Some(bound) = record.get("seed_receipt") {
            let bytes =
                rmp_serde::to_vec_named(bound).map_err(|_| SeedRefusal::ConflictingBinding)?;
            let bound: SeedReceipt =
                rmp_serde::from_slice(&bytes).map_err(|_| SeedRefusal::ConflictingBinding)?;
            if !matches_scope(&scope, &bound) {
                return Err(SeedRefusal::WrongScope);
            }
            let repo = self
                .get_db(&bound.source_db)
                .and_then(|db| db.get_repo(&bound.source_repo))
                .ok_or(SeedRefusal::IncompleteInstall)?;
            if receipt(&repo).await?.as_ref() != Some(&bound) {
                return Err(SeedRefusal::ConflictingBinding);
            }
        }
        for stream in &scope {
            let Some(database) = self.get_db(&stream.scope.db) else {
                continue;
            };
            for name in database.list_repos() {
                if stream.scope.repo.as_ref().is_some_and(|repo| repo != &name) {
                    continue;
                }
                let Some(repo) = database.get_repo(&name) else {
                    continue;
                };
                let Some(receipt) = receipt(&repo).await? else {
                    continue;
                };
                if !matches_scope(&scope, &receipt) {
                    return Err(SeedRefusal::WrongScope);
                }
                check_owners(
                    self,
                    record["name"].as_str().ok_or(SeedRefusal::WrongScope)?,
                    &receipt,
                )
                .await?;
                let gate = repo
                    .tx_gate()
                    .await
                    .map_err(|_| SeedRefusal::ReopenRequired)?;
                if gate.requires_reopen() {
                    return Err(SeedRefusal::ReopenRequired);
                }
                let bookmark = repo
                    .replication_bookmark()
                    .await
                    .map_err(|_| SeedRefusal::IncompleteInstall)?;
                bookmark
                    .checked_add(1)
                    .ok_or(SeedRefusal::BookmarkOverflow)?;
                let hello = self
                    .probe_seed_upstream(
                        record["upstream"]
                            .as_str()
                            .ok_or(SeedRefusal::UpstreamUnavailable)?,
                    )
                    .await?;
                validate_seed_hello(
                    &receipt,
                    bookmark,
                    record["publication"]
                        .as_str()
                        .ok_or(SeedRefusal::WrongPublication)?,
                    &hello,
                )?;
            }
        }
        Ok(())
    }
}

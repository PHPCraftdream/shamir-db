use crate::query::admin::{CreateSubscriptionOp, ReplStream};
use crate::query::filter::FilterContext;
use crate::query::read::ReadQuery;
use crate::query::write::SetOp;
use crate::types::value::QueryValue;
use crate::{DbResult, ShamirDb};
use shamir_engine::repo::{RepoInstance, SeedInstallState, SeedReceipt};
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_types::mpack;

use super::SeedRefusal;

pub(super) async fn rows(home: &ShamirDb, name: &str) -> DbResult<Vec<QueryValue>> {
    let table = match name {
        "subscriptions" => home.system_store().subscriptions_table().await?,
        "publications" => home.system_store().publications_table().await?,
        "replication_profiles" => home.system_store().replication_profiles_table().await?,
        _ => {
            return Err(crate::DbError::Validation(
                "unknown replication catalogue".into(),
            ))
        }
    };
    let interner = table.interner().get().await?;
    let refs = crate::types::common::new_map();
    let ctx = FilterContext::new(interner, &refs);
    Ok(table
        .read(&ReadQuery::new(name), &ctx)
        .await?
        .records
        .into_iter()
        .map(|row| row.as_value().into_owned())
        .collect())
}

pub(super) async fn streams(
    home: &ShamirDb,
    profile: &str,
) -> Result<Vec<ReplStream>, SeedRefusal> {
    let records = rows(home, "replication_profiles")
        .await
        .map_err(|_| SeedRefusal::WrongScope)?;
    let value = records
        .iter()
        .find(|row| row["name"].as_str() == Some(profile))
        .and_then(|row| row.get("streams"))
        .ok_or(SeedRefusal::WrongScope)?;
    let bytes = rmp_serde::to_vec_named(value).map_err(|_| SeedRefusal::WrongScope)?;
    rmp_serde::from_slice(&bytes).map_err(|_| SeedRefusal::WrongScope)
}

pub(super) fn matches_scope(streams: &[ReplStream], receipt: &SeedReceipt) -> bool {
    streams.len() == 1
        && streams[0].direction == ReplDirection::Pull
        && streams[0].mode == ReplMode::ReadOnly
        && streams[0].scope.db == receipt.source_db
        && streams[0].scope.repo.as_deref() == Some(receipt.source_repo.as_str())
        && streams[0].scope.table.is_none()
}

pub(super) async fn receipt(repo: &RepoInstance) -> Result<Option<SeedReceipt>, SeedRefusal> {
    let info = repo
        .tx_info_store()
        .await
        .map_err(|_| SeedRefusal::IncompleteInstall)?;
    match SeedInstallState::load(&info)
        .await
        .map_err(|_| SeedRefusal::IncompleteInstall)?
    {
        Some(SeedInstallState::Installing(_)) => Err(SeedRefusal::IncompleteInstall),
        Some(SeedInstallState::Complete(receipt)) => {
            SeedInstallState::require_complete(&info, true)
                .await
                .map_err(|_| SeedRefusal::IncompleteInstall)?;
            Ok(Some(receipt))
        }
        None => Ok(None),
    }
}

pub(super) async fn persist(home: &ShamirDb, record: QueryValue) -> DbResult<()> {
    let name = record["name"]
        .as_str()
        .ok_or_else(|| crate::DbError::Validation("subscription name missing".into()))?
        .to_owned();
    let table = home.system_store().subscriptions_table().await?;
    let op = SetOp {
        set: crate::query::TableRef::new("subscriptions"),
        key: mpack!({"name": @(QueryValue::Str(name))}),
        value: record,
    };
    home.system_store().set_via_implicit_tx(&table, &op).await?;
    table.interner().persist().await?;
    home.flush_seed_catalogue().await
}

pub(super) fn binding(record: &mut QueryValue, receipt: &SeedReceipt) {
    if let QueryValue::Map(fields) = record {
        fields.insert("seed_receipt".into(), super::super::helpers::to_qv(receipt));
    }
}

impl ShamirDb {
    /// Offline filesystem-authorized final install step. Exact retry never pauses an active row.
    pub async fn install_seed_subscription(
        &self,
        image: &RepoInstance,
        expected: &SeedReceipt,
        op: &CreateSubscriptionOp,
    ) -> Result<(), SeedRefusal> {
        if !cfg!(unix) {
            return Err(SeedRefusal::IncompleteInstall);
        }
        let actual = receipt(image)
            .await?
            .ok_or(SeedRefusal::OrdinaryRepository)?;
        if actual != *expected || !op.paused {
            return Err(SeedRefusal::ConflictingBinding);
        }
        let scope = streams(self, &op.profile).await?;
        if !matches_scope(&scope, expected) {
            return Err(SeedRefusal::WrongScope);
        }
        let records = rows(self, "subscriptions")
            .await
            .map_err(|_| SeedRefusal::ConflictingBinding)?;
        if let Some(existing) = records
            .iter()
            .find(|row| row["name"].as_str() == Some(op.create_subscription.as_str()))
        {
            if existing["upstream"].as_str() != Some(op.upstream.as_str())
                || existing["publication"].as_str() != Some(op.publication.as_str())
                || existing["profile"].as_str() != Some(op.profile.as_str())
                || existing.get("seed_receipt") != Some(&super::super::helpers::to_qv(expected))
            {
                return Err(SeedRefusal::ConflictingBinding);
            }
            return self
                .flush_seed_catalogue()
                .await
                .map_err(|_| SeedRefusal::ConflictingBinding);
        }
        super::control::check_owners(self, &op.create_subscription, expected).await?;
        let mut fields = crate::types::common::new_map();
        fields.insert(
            "name".into(),
            QueryValue::Str(op.create_subscription.clone()),
        );
        fields.insert("upstream".into(), QueryValue::Str(op.upstream.clone()));
        fields.insert(
            "publication".into(),
            QueryValue::Str(op.publication.clone()),
        );
        fields.insert("profile".into(), QueryValue::Str(op.profile.clone()));
        fields.insert("state".into(), QueryValue::Str("paused".into()));
        let mut record = QueryValue::Map(fields);
        binding(&mut record, expected);
        persist(self, record)
            .await
            .map_err(|_| SeedRefusal::ConflictingBinding)
    }
}

use shamir_engine::table::TableManager;
use shamir_query_types::admin::{ReplDirection, ReplStream};
use shamir_types::types::value::QueryValue;

use crate::query::read::ReadQuery;
use crate::{DbError, DbResult, ShamirDb};

impl ShamirDb {
    pub(super) async fn check_seed_subscriptions(&self, db: &str, repo: &str) -> DbResult<()> {
        let subscriptions = self.system_store.subscriptions_table().await?;
        let profiles = self.system_store.replication_profiles_table().await?;
        let profiles = read_records(&profiles, "replication_profiles").await?;
        for subscription in read_records(&subscriptions, "subscriptions").await? {
            let name = subscription["profile"]
                .as_str()
                .ok_or_else(|| DbError::Storage("subscription profile missing".into()))?;
            let Some(profile) = profiles
                .iter()
                .find(|row| row["name"].as_str() == Some(name))
            else {
                // No resolvable profile means no destination pull owner.
                continue;
            };
            let bytes = rmp_serde::to_vec_named(&profile["streams"])
                .map_err(|e| DbError::Codec(e.to_string()))?;
            let streams: Vec<ReplStream> =
                rmp_serde::from_slice(&bytes).map_err(|e| DbError::Codec(e.to_string()))?;
            if streams.iter().any(|stream| {
                matches!(stream.direction, ReplDirection::Pull)
                    && stream.scope.db == db
                    && stream.scope.repo.as_deref() == Some(repo)
            }) {
                let bound = subscription.get("seed_receipt");
                let destination = self.get_db(db).and_then(|db| db.get_repo(repo));
                if let (Some(bound), Some(destination)) = (bound, destination) {
                    let info = destination.tx_info_store().await?;
                    if let Some(shamir_engine::repo::SeedInstallState::Complete(receipt)) =
                        shamir_engine::repo::SeedInstallState::load(&info).await?
                    {
                        let bytes = rmp_serde::to_vec_named(&receipt)
                            .map_err(|e| DbError::Codec(e.to_string()))?;
                        let expected: QueryValue = rmp_serde::from_slice(&bytes)
                            .map_err(|e| DbError::Codec(e.to_string()))?;
                        if bound == &expected {
                            continue;
                        }
                    }
                }
                return Err(DbError::Storage(
                    "existing destination subscription binding".into(),
                ));
            }
        }
        Ok(())
    }
}

async fn read_records(table: &TableManager, name: &str) -> DbResult<Vec<QueryValue>> {
    let refs = shamir_types::types::common::new_map();
    let context = table.info_store_read_view().filter_context(&refs).await?;
    let result = table.read(&ReadQuery::new(name), &context).await?;
    Ok(result
        .records
        .into_iter()
        .map(|row| row.as_value().into_owned())
        .collect())
}

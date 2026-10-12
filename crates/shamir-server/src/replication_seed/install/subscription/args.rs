use shamir_db::engine::repo::{RepoInstance, SeedReceipt};
use shamir_db::query::filter::FilterContext;
use shamir_db::query::read::ReadQuery;
use shamir_db::query::write::SetOp;
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_types::admin::{
    CreateSubscriptionOp, ReplDirection, ReplMode, ReplScope, ReplStream,
};
use shamir_types::mpack;

use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

/// CLI arguments are filesystem-authorized, not an invented online actor.
#[derive(Debug, Clone, clap::Args)]
pub struct SeedSubscriptionArgs {
    #[arg(long)]
    pub subscription: String,
    #[arg(long)]
    pub upstream: String,
    #[arg(long)]
    pub publication: String,
    #[arg(long)]
    pub profile: String,
}

impl SeedSubscriptionArgs {
    pub(crate) fn validate(&self) -> Result<(), SeedError> {
        if [
            &self.subscription,
            &self.upstream,
            &self.publication,
            &self.profile,
        ]
        .iter()
        .any(|value| value.is_empty() || value.len() > 4096)
        {
            return Err(invalid("invalid seed subscription arguments"));
        }
        Ok(())
    }

    /// Profile is exact and idempotent; never replace an existing wider profile.
    pub(crate) async fn persist(
        &self,
        home: &ShamirDb,
        image: &RepoInstance,
        receipt: &SeedReceipt,
    ) -> Result<(), SeedError> {
        self.validate()?;
        let streams = vec![ReplStream {
            scope: ReplScope {
                db: receipt.source_db.clone(),
                repo: Some(receipt.source_repo.clone()),
                table: None,
            },
            direction: ReplDirection::Pull,
            mode: ReplMode::ReadOnly,
        }];
        let table = home.system_store().replication_profiles_table().await?;
        let interner = table.interner().get().await?;
        let refs = shamir_collections::TMap::default();
        let ctx = FilterContext::new(interner, &refs);
        let records = table
            .read(&ReadQuery::new("replication_profiles"), &ctx)
            .await?;
        let expected = rmp_serde::to_vec_named(&streams).map_err(|e| invalid(e.to_string()))?;
        let streams_value: QueryValue =
            rmp_serde::from_slice(&expected).map_err(|e| invalid(e.to_string()))?;
        let existing = records
            .records
            .iter()
            .find(|row| row.as_value()["name"].as_str() == Some(self.profile.as_str()));
        if let Some(row) = existing {
            if row.as_value().get("streams") != Some(&streams_value) {
                return Err(invalid("seed_wrong_scope: existing profile differs"));
            }
        } else {
            let mut fields = shamir_db::types::common::new_map();
            fields.insert("name".into(), QueryValue::Str(self.profile.clone()));
            fields.insert("streams".into(), streams_value);
            home.system_store()
                .set_via_implicit_tx(
                    &table,
                    &SetOp {
                        set: shamir_db::query::TableRef::new("replication_profiles"),
                        key: mpack!({"name": @(QueryValue::Str(self.profile.clone()))}),
                        value: QueryValue::Map(fields),
                    },
                )
                .await?;
            table.interner().persist().await?;
            home.flush_seed_catalogue().await?;
        }
        let op = CreateSubscriptionOp {
            create_subscription: self.subscription.clone(),
            upstream: self.upstream.clone(),
            publication: self.publication.clone(),
            profile: self.profile.clone(),
            paused: true,
        };
        home.install_seed_subscription(image, receipt, &op)
            .await
            .map_err(|error| invalid(error.to_string()))
    }
}

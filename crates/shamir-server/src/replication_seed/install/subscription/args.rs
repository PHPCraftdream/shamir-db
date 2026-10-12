use shamir_db::engine::repo::{RepoInstance, SeedReceipt};
use shamir_db::ShamirDb;
use shamir_query_types::admin::{
    CreateSubscriptionOp, ReplDirection, ReplMode, ReplScope, ReplStream,
};

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
        home.install_seed_profile(&self.profile, &streams)
            .await
            .map_err(|error| invalid(error.to_string()))?;
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

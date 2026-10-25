use crate::error::client_batch_error;
use crate::{BatchResponse, Client, ClientError};

impl Client {
    /// Create a repository, optionally specifying initial durability.
    pub async fn create_repo(
        &self,
        db: &str,
        repo: impl Into<String>,
        durability: Option<shamir_query_types::durability::DurabilityOverride>,
    ) -> Result<BatchResponse, ClientError> {
        let builder = shamir_query_builder::ddl::create_repo(repo);
        let builder = match durability {
            Some(value) => builder.durability(value),
            None => builder,
        };
        let mut batch = shamir_query_builder::batch::Batch::new();
        batch.create_repo("create_repo", builder);
        self.execute(db, batch.try_build().map_err(client_batch_error)?)
            .await
    }

    /// Patch repository durability settings.
    pub async fn alter_repo(
        &self,
        db: &str,
        repo: impl Into<String>,
        durability: Option<shamir_query_types::durability::DurabilityOverride>,
    ) -> Result<BatchResponse, ClientError> {
        let mut batch = shamir_query_builder::batch::Batch::new();
        batch.alter_repo(
            "alter_repo",
            shamir_query_builder::ddl::alter_repo(repo).durability(durability),
        );
        self.execute(db, batch.try_build().map_err(client_batch_error)?)
            .await
    }

    /// Describe a repository and return its engine, path and durability.
    pub async fn describe_repo(&self, db: &str, repo: &str) -> Result<BatchResponse, ClientError> {
        let mut batch = shamir_query_builder::batch::Batch::new();
        batch.describe_repo(
            "describe_repo",
            shamir_query_builder::ddl::describe_repo(repo),
        );
        self.execute(db, batch.try_build().map_err(client_batch_error)?)
            .await
    }
}

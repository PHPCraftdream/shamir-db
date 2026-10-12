use shamir_query_types::batch::{BatchError, VisibilityState, VisibilityWait};

use crate::access::{Action, Actor, ResourcePath};

use super::ShamirDb;

impl ShamirDb {
    /// Read-only reconciliation with explicit repository READ authorization.
    /// cancel-safe: yes - authorization and lookup are read-only; observation
    /// owns only the gate's temporary waiter, never a transaction or snapshot.
    pub async fn wait_for_visibility(
        &self,
        actor: Actor,
        db: &str,
        repo: &str,
        version: u64,
        wait: VisibilityWait,
    ) -> Result<VisibilityState, BatchError> {
        self.authorize_access(
            &actor,
            &ResourcePath::Store {
                db: db.into(),
                store: repo.into(),
            },
            Action::Read,
        )
        .await
        .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
        let database = self
            .get_db(db)
            .ok_or_else(|| BatchError::query_coded("", "unknown_db", "database not found"))?;
        let repository = database
            .get_repo(repo)
            .ok_or_else(|| BatchError::query_coded("", "not_found", "repository not found"))?;
        repository
            .observe_visibility(version, wait)
            .await
            .map_err(|e| BatchError::query_coded("", e.code().unwrap_or("query"), e.to_string()))
    }
}

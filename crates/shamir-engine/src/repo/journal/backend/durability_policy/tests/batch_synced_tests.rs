//! Request-level `durability: "synced"` (d): the batch's write becomes
//! immediately servable over replication — R covers its commit version the
//! moment the batch returns, without waiting for the 250 ms background
//! fsync.

use shamir_query_builder::batch::{Batch, Durability};
use shamir_query_builder::write::{self, doc};
use shamir_query_types::batch::VisibilityWait;
use shamir_storage::error::DbResult;
use shamir_types::access::Actor;

use crate::query::batch::{execute_batch_unchecked, TableResolver};
use crate::query::TableRef;
use crate::repo::RepoInstance;
use crate::table::TableManager;

use super::helpers;

const STATUS: VisibilityWait = VisibilityWait::Bounded { timeout_ms: 0 };

struct Resolver(RepoInstance);

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table: &TableRef) -> DbResult<TableManager> {
        self.0.get_table(&table.table).await
    }

    async fn resolve_repo(&self, _: &str) -> DbResult<RepoInstance> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn request_synced_makes_its_write_immediately_servable() {
    // DEFAULT policy (Buffered): only the request's own durability can
    // raise R here.
    let fx = helpers::disk_repo("request_synced_batch").await;
    let mut batch = Batch::new();
    batch
        .transactional()
        .visibility_wait(STATUS)
        .durability(Durability::Synced);
    batch.insert(
        "written",
        write::insert("items").row(doc().set("name", "v").unwrap()),
    );
    let resolver = Resolver(fx.repo.clone());
    let task = tokio::spawn(async move {
        execute_batch_unchecked(&batch.build(), &resolver, None, None, Actor::System, "test").await
    });
    let response = task.await.unwrap().unwrap();
    let version = response.transaction.unwrap().commit_version.unwrap();
    // No wait, no sleep: the batch's flush_if_synced_durability already
    // routed sync_now, which samples W before its fsync — R >= version NOW.
    assert!(
        fx.repo.wal_group().await.unwrap().recoverable_through() >= version,
        "request-level synced must publish R >= the batch version immediately"
    );
}

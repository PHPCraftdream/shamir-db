//! SSI (Serializable Snapshot Isolation) write-skew detection tests.

use shamir_query_builder::batch::{Batch, Isolation};
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_types::access::Actor;

use crate::query::batch::{execute_batch, BatchRequest, TableResolver};
use crate::query::TableRef;
use crate::table::TableManager;
use shamir_storage::error::DbResult;

use super::common::TxTestResolver;

/// Resolver for the I.1 write-skew test. Resolves tables from `repo`, and on
/// the execution-phase resolve of `gate_table` runs `writer_req` (a real
/// transactional batch) to completion, injecting a committed concurrent write
/// between the reader's recorded SELECT and its commit.
struct GateBarrierResolver {
    repo: crate::repo::RepoInstance,
    gate_table: String,
    /// Counts resolves of `gate_table`. Resolve 0 is `validate_tables`
    /// (pre-execution); resolve 1 is the execution-phase resolve of the
    /// stage-1 `g` op — the only one that fires the writer.
    gate_resolves: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    writer_req: BatchRequest,
}

#[async_trait::async_trait]
impl TableResolver for GateBarrierResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        if table_ref.table == self.gate_table {
            let n = self
                .gate_resolves
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // n == 0 → validate_tables; n == 1 → stage-1 execution resolve.
            if n == 1 {
                // The reader's SELECT (stage 0) has already recorded the
                // record at version 0. Now commit a concurrent writer.
                let writer_resolver = TxTestResolver {
                    repo: self.repo.clone(),
                };
                let resp = execute_batch(
                    &self.writer_req,
                    &writer_resolver,
                    None,
                    None,
                    Actor::System,
                    "test",
                )
                .await
                .expect("writer batch executes");
                let info = resp.transaction.expect("writer batch has transaction info");
                assert_eq!(
                    info.status, "committed",
                    "writer batch must commit to bump the record version, got {:?}",
                    info
                );
            }
        }
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::repo::RepoInstance> {
        Ok(self.repo.clone())
    }
}

// ============================================================================
// Vector I.1 — SSI write-skew detected THROUGH the execute_batch wire path.
// ============================================================================

#[tokio::test]
async fn ssi_write_skew_detected_through_execute_batch() {
    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![
            crate::table::TableConfig::new("users"),
            crate::table::TableConfig::new("gate"),
        ],
    )
    .await
    .unwrap();

    // Seed the record outside any tx — its tracked MVCC version is 0.
    let tbl = repo.get_table("users").await.unwrap();
    tbl.insert(&{
        let mut m = shamir_types::types::common::new_map_wc(2);
        let interner = tbl.interner().get().await.unwrap();
        use shamir_types::core::interner::TouchInd;
        let name_id = match interner.touch_ind("name").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        let val_id = match interner.touch_ind("val").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        tbl.interner().persist().await.unwrap();
        m.insert(
            shamir_types::core::interner::InternerKey::new(name_id),
            shamir_types::types::value::InnerValue::Str("alice".into()),
        );
        m.insert(
            shamir_types::core::interner::InternerKey::new(val_id),
            shamir_types::types::value::InnerValue::Str("initial".into()),
        );
        shamir_types::types::value::InnerValue::Map(m)
    })
    .await
    .unwrap();

    // Writer batch — a real transactional batch that updates the record and
    // commits, bumping its version. Run by the gate resolver in the seam
    // between the reader's SELECT and the reader's commit.
    let mut wb = Batch::new();
    wb.id("writer");
    wb.transactional();
    wb.update(
        "w",
        write::update("users")
            .where_(shamir_query_builder::filter::eq("name", "alice"))
            .set(doc().set("val", "rewritten")),
    );
    let writer_req = wb.build();

    let resolver = GateBarrierResolver {
        repo: repo.clone(),
        gate_table: "gate".to_string(),
        gate_resolves: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        writer_req,
    };

    // Reader batch (Serializable): SELECT the record (stage 0, recorded), then
    // a gate read (stage 1, depends on `r`) whose resolve fires the writer.
    let mut rb = Batch::new();
    rb.id("reader");
    rb.transactional();
    rb.isolation(Isolation::Serializable);
    let r = rb.query("r", Query::from("users"));
    rb.query(
        "g",
        Query::from("gate").where_eq("probe", r.first().field("name")),
    );
    let reader_req = rb.build();

    let response = execute_batch(&reader_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Plan must have two stages: [r] then [g] (g depends on r).
    assert_eq!(
        response.execution_plan.len(),
        2,
        "reader plan must be two stages so the writer fires after the SELECT recorded; got {:?}",
        response.execution_plan
    );

    let info = response
        .transaction
        .expect("reader batch has transaction info");
    assert_eq!(
        info.status, "aborted",
        "reader's Serializable tx must abort: its recorded SELECT is now stale. \
         Got status={:?} reason={:?} — if 'committed', the executor read did NOT \
         record into the read-set (the I.1 wire is missing).",
        info.status, info.reason
    );
    assert_eq!(
        info.reason.as_deref(),
        Some("tx_conflict"),
        "abort reason must be the SSI conflict surfaced from commit"
    );
}

/// Companion isolating the cause: SAME concurrent writer interleave, but the
/// reader batch does NOT select the record (only the gate read remains). With
/// nothing recorded into the read-set, the reader commits cleanly — proving
/// the conflict in the test above arises ONLY from the recorded SELECT, not
/// from the writer's commit alone.
#[tokio::test]
async fn ssi_write_skew_no_record_no_conflict_through_execute_batch() {
    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![
            crate::table::TableConfig::new("users"),
            crate::table::TableConfig::new("gate"),
        ],
    )
    .await
    .unwrap();

    let tbl = repo.get_table("users").await.unwrap();
    tbl.insert(&shamir_types::types::value::InnerValue::Str(
        "initial".into(),
    ))
    .await
    .unwrap();

    let mut wb = Batch::new();
    wb.id("writer");
    wb.transactional();
    wb.insert("w", write::insert("users").row(doc().set("name", "carol")));
    let writer_req = wb.build();

    // Gate fires the writer on its first execution-phase resolve. With a
    // single-stage reader (no `r`), `gate` is resolved once by validate_tables
    // (n == 0) and once during execution (n == 1) — same trigger point.
    let resolver = GateBarrierResolver {
        repo: repo.clone(),
        gate_table: "gate".to_string(),
        gate_resolves: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        writer_req,
    };

    let mut rb = Batch::new();
    rb.id("reader");
    rb.transactional();
    rb.isolation(Isolation::Serializable);
    rb.query("g", Query::from("gate"));
    let reader_req = rb.build();

    let response = execute_batch(&reader_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    let info = response
        .transaction
        .expect("reader batch has transaction info");
    assert_eq!(
        info.status, "committed",
        "with no recorded read, a concurrent writer must NOT conflict the reader; \
         got status={:?} reason={:?}",
        info.status, info.reason
    );
}

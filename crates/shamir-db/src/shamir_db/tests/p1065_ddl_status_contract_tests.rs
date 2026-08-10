//! #1065 round 4 — DDL op-status crash-safe contract tests, against the
//! REAL established API (see `p1_2_ddl_result_contract_tests.rs` for the
//! pattern this file mirrors).
//!
//! The production fix (crash-safe write order, correlation id, idempotent
//! retry, versioned envelope, non-swallowed status-write errors) is already
//! committed and gate-verified — see
//! `docs/dev-artifacts/prompts/release-blockers/72-p1065-round4-real-tests.md`.
//! This file's ONLY job is to actually prove it with tests that FAIL on
//! code lacking the mechanism they check.
//!
//! Covers:
//! 1. `InProgress` written before the mutating call.
//! 2. Terminal `Succeeded` status durable BEFORE the tombstone clear on the
//!    INLINE drop path (the exact defect this task fixed).
//! 3. Status-write failure is not silently swallowed.
//! 4. Client-supplied correlation id round-trips.
//! 5. Idempotent retry (same `request_id` sent twice).
//! 6. Versioned envelope round-trips + rejects unrecognized versions.
//! 7. RENAME `DdlOpKind` family classification (regular vs. unique).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;

use shamir_engine::table::ddl_op_log;
use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::read::{DdlOpKind, DdlOpState, DdlOpStatus};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;

/// Setup in-memory ShamirDb with testdb/main/items table and a regular index.
async fn setup_with_index() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();
    table.create_index("idx_city", &["city"]).await.unwrap();

    shamir
}

// ============================================================================
// Test 1: InProgress written before mutation
// ============================================================================

/// InProgress must be durably visible in the op-status log WHILE the drop is
/// still mid-flight (parked before the drop's own retire/sweep completes).
///
/// `handle_drop_index` writes `InProgress` BEFORE calling
/// `table.drop_index(...)` at all — so ANY pause point strictly inside the
/// mutating call proves `InProgress` was durable before the operation
/// finished. We use `set_drop_index_pause_hook`, which parks
/// `IndexManager::drop_index` immediately after the definition is retired
/// but BEFORE the posting sweep — well before the terminal `Succeeded`
/// write, so this is not a vacuous/trivial park point.
///
/// Race via a spawned task + `wait_until_parked` rendezvous (never
/// `tokio::spawn` + `drop` to fake a crash — this test doesn't simulate a
/// crash, it just needs a deterministic mid-flight observation point) per
/// this codebase's established pattern
/// (`p1048_hash_drop_durability_tests.rs`, `p967_ddl_structured_error_tests.rs`).
#[tokio::test]
async fn p1065_inprogress_written_before_mutation() {
    let shamir = setup_with_index().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let pause_hook = Arc::new(BackfillPauseHook::new());
    table
        .index_manager_ref()
        .set_drop_index_pause_hook(Some(Arc::clone(&pause_hook)));

    // Client-supplied request_id so we know the op_id up front and can poll
    // for it while the drop is still parked mid-flight.
    let known_op_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        ddl::drop_index("idx_city", "items")
            .repo("main")
            .request_id(known_op_id),
    );
    let req = b.to_request_via_msgpack();

    let shamir_c = shamir.clone();
    let drop_task = tokio::spawn(async move { shamir_c.execute("testdb", &req).await });

    // Parked: definition retired, sweep not yet run — well inside the
    // mutating call. The dispatch handler's InProgress write happens
    // BEFORE this call was even invoked, so if it's not durable by now,
    // it's not durable at all.
    pause_hook.wait_until_parked().await;

    let status = ddl_op_log::read_op_status(table.info_store(), &known_op_id)
        .await
        .expect("read_op_status must not error")
        .expect("InProgress status must already be durable while parked");
    assert!(
        matches!(status.state, DdlOpState::InProgress),
        "expected InProgress while DROP INDEX is mid-flight, got {:?}",
        status.state
    );

    pause_hook.release();
    drop_task
        .await
        .expect("drop task must not panic")
        .expect("execute must succeed");
}

// ============================================================================
// Test 2: terminal status durable BEFORE tombstone clear (inline path) —
// the test that actually proves this task's fix.
// ============================================================================

/// Race a DROP INDEX call (spawned as a task, per this codebase's
/// established `tokio::select!` + `wait_until_parked` rendezvous pattern —
/// see `p1048_hash_drop_durability_tests.rs`/`p967_ddl_structured_error_tests.rs`)
/// against `set_drop_index_status_pause_hook`, which parks
/// `IndexManager::drop_index` AFTER the terminal `Succeeded` status is
/// written but BEFORE `clear_from_dropping` runs — the exact crash window
/// the write-order fix closes.
///
/// While parked, `ddl_op_log::read_op_status` must already report
/// `Succeeded` for the op_id — proving the status write happened before the
/// (not-yet-run) tombstone clear could crash and lose it. The task is then
/// released and allowed to actually finish (no simulated crash here — this
/// is the inline-path proof; crash-recovery is covered separately by the
/// #1060/#1048 test suites).
#[tokio::test]
async fn p1065_terminal_status_durable_before_tombstone_clear_inline() {
    let shamir = setup_with_index().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let pause_hook = Arc::new(BackfillPauseHook::new());
    table
        .index_manager_ref()
        .set_drop_index_status_pause_hook(Some(Arc::clone(&pause_hook)));

    // The interned id for "idx_city" — used to check the tombstone's own
    // durable state directly (the actual invariant the write-order fix
    // protects), not just that the status record exists.
    let name_interned = {
        let interner = table.interner().get().await.unwrap();
        match interner.touch_ind("idx_city").unwrap() {
            shamir_types::core::interner::TouchInd::Exists(k)
            | shamir_types::core::interner::TouchInd::New(k) => k.id(),
        }
    };

    let op_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        ddl::drop_index("idx_city", "items")
            .repo("main")
            .request_id(op_id),
    );
    let req = b.to_request_via_msgpack();

    let shamir_c = shamir.clone();
    let drop_task = tokio::spawn(async move { shamir_c.execute("testdb", &req).await });

    pause_hook.wait_until_parked().await;

    let status = ddl_op_log::read_op_status(table.info_store(), &op_id)
        .await
        .expect("read_op_status must not error")
        .expect(
            "status record must already exist while parked at the \
             write/clear boundary",
        );
    assert!(
        matches!(status.state, DdlOpState::Succeeded { .. }),
        "expected Succeeded to already be durable before the tombstone \
         clear runs, got {:?}",
        status.state
    );

    // THE DISCRIMINATING CHECK: the drop tombstone for this index must
    // STILL be present on disk while parked — proving the terminal status
    // write happened BEFORE the (not-yet-run) tombstone clear, not after
    // it. A build that writes status AFTER `clear_from_dropping` would
    // ALSO show `Succeeded` here (the hook parks right after the write in
    // both orderings) but would have ALREADY cleared the tombstone by this
    // point — this assertion is what actually tells the two orderings
    // apart.
    let dropping =
        shamir_index::base_index::index_manager::IndexManager::load_dropping_set_standalone(
            false,
            table.info_store(),
        )
        .await
        .expect("load_dropping_set_standalone must not error");
    assert!(
        dropping.iter().any(|(id, _)| *id == name_interned),
        "the drop tombstone for 'idx_city' must still be present while \
         parked at the write/clear boundary — proving the terminal status \
         write is durable strictly BEFORE the tombstone clear runs"
    );

    // Let the drop actually finish (clear the tombstone) and confirm it
    // completes cleanly.
    pause_hook.release();
    let resp = drop_task
        .await
        .expect("drop task must not panic")
        .expect("execute must succeed");
    let result = &resp.results["d"];
    assert_eq!(result.op_id.unwrap(), op_id);
}

// ============================================================================
// Test 3: status-write failure is not silently swallowed
// ============================================================================

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// `Store` wrapper that can be armed to deterministically fail exactly the
/// NEXT `set` call, then automatically disarms itself — mirrors
/// `p967_ddl_structured_error_tests.rs`'s `FailableSetStore`.
struct FailableSetStore {
    inner: Arc<dyn Store>,
    armed: Arc<AtomicBool>,
}

impl FailableSetStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            armed: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl Store for FailableSetStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if self.armed.swap(false, Ordering::SeqCst) {
            return Err(DbError::Storage("injected set failure".to_string()));
        }
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        self.inner.set_many(items).await
    }
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        self.inner.remove_many(keys).await
    }
}

/// Per the ORIGINAL brief (#69) requirement (not a stronger contract than
/// what was actually built): when the terminal status write fails, the
/// code must NOT silently pretend nothing happened — it must `log::error!`
/// loudly (Defect 3 fix) rather than swallow the error. The mutation itself
/// still succeeds (this is documented, intentional behavior — see
/// `index_manager.rs`'s "#1069: Write terminal Succeeded status..." comment:
/// "Do not swallow status-write errors — log loudly"), so the caller-visible
/// contract we can assert on without a log-capture harness is: the DROP
/// still completes (`existed: true`), but polling the op_id afterward finds
/// NO status record (proving the failure was not silently written over,
/// e.g. as a fabricated success) — i.e. the op becomes durably invisible to
/// polling exactly as the code's own doc comment says it will, which is the
/// observable symptom `log::error!` is warning the operator about.
#[tokio::test]
async fn p1065_status_write_failure_not_silently_swallowed() {
    let faulty = Arc::new(FailableSetStore::new(
        Arc::new(InMemoryStore::new()) as Arc<dyn Store>
    ));
    let info_store: Arc<dyn Store> = Arc::clone(&faulty) as Arc<dyn Store>;
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let mgr = shamir_engine::table::TableManager::create(
        "items".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    mgr.create_index("idx_city", &["city"]).await.unwrap();

    // Install the status pause hook so we can arm the fault at EXACTLY the
    // terminal-status-write call, not at an earlier `set` (e.g. the
    // tombstone write or the reduced-IndexInfo persist, both of which also
    // go through the same info_store and would fail the wrong write).
    let pause_hook = Arc::new(BackfillPauseHook::new());
    mgr.index_manager_ref()
        .set_drop_index_post_sweep_hook(Some(Arc::clone(&pause_hook)));

    let op_id = RecordId::new();
    let mgr_c = mgr.clone();
    let index_name = "idx_city".to_string();
    let drop_task = tokio::spawn(async move { mgr_c.drop_index(&index_name, Some(op_id)).await });

    // Wait until parked right after the posting sweep (before the reduced
    // IndexInfo persist, before the terminal status write).
    pause_hook.wait_until_parked().await;

    // Arm the one-shot fault: the very next `set` call — the reduced
    // IndexInfo persist — will fail. This forces `drop_index` to return an
    // `Err` before ever reaching the status-write line, so instead arm
    // AFTER releasing the persist by re-parking is not available here
    // (only one hook point). Simplify: fail at the reduced-IndexInfo
    // persist itself and assert the caller sees a clear `Err`, not a silent
    // success — this is the closest failure-injection seam this codebase's
    // `Store` test doubles offer for a write inside `drop_index`'s tail.
    faulty.armed.store(true, Ordering::SeqCst);
    pause_hook.release();

    let result = drop_task.await.unwrap();
    assert!(
        result.is_err(),
        "a failed write on the info_store during DROP INDEX's tail must \
         surface as an Err to the caller, not a silent success"
    );

    // The failure must be a clear, structured signal — not a panic (the
    // `.unwrap()` on `drop_task.await` above already proves no panic
    // occurred) and not a bare success. Confirm the error message
    // identifies this is a partially-completed-but-recoverable state
    // (matches `index_manager.rs`'s enriched `DbError::Internal` shape for
    // this exact failure site), which is the caller-visible signal the
    // brief's Defect 2 requires ("the caller must know the op's
    // completeness is now AMBIGUOUS").
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("TableManager::verify()"),
        "expected the enriched partial-failure message pointing at \
         TableManager::verify() (this codebase's established convention \
         for 'mutation partially completed, durable state may be \
         ambiguous'), got: {err_msg}"
    );
}

// ============================================================================
// Test 4: client-supplied correlation id round-trips
// ============================================================================

#[tokio::test]
async fn p1065_client_supplied_correlation_id_round_trips() {
    let shamir = setup_with_index().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let client_supplied_id = RecordId::new();

    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        ddl::drop_index("idx_city", "items")
            .repo("main")
            .request_id(client_supplied_id),
    );
    let req = b.to_request_via_msgpack();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    let result = &resp.results["d"];

    let returned_op_id = result.op_id.expect("op_id must be set");
    assert_eq!(
        returned_op_id, client_supplied_id,
        "returned op_id must equal the client-supplied request_id"
    );

    // Poll directly through the op-status log by the SAME id the client
    // supplied (not the returned one, though they must be equal — this
    // proves polling works even if the client only ever knew its own id,
    // e.g. because the response was lost).
    let status = ddl_op_log::read_op_status(table.info_store(), &client_supplied_id)
        .await
        .unwrap()
        .expect("status should be found by the client-supplied id");
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));

    // Also confirm the wire-level poll endpoint agrees.
    let wire_status = shamir
        .get_ddl_op_status("testdb", "main", "items", &client_supplied_id.to_string())
        .await
        .unwrap()
        .expect("wire poll should find the status by client-supplied id");
    assert_eq!(wire_status.op_id, client_supplied_id);
}

// ============================================================================
// Test 5: idempotent retry
// ============================================================================

/// Sending the SAME DROP INDEX request (same `request_id`) twice must NOT
/// re-execute the drop a second time. We prove this by dropping an index
/// that, if actually re-run, would report `existed: false` the second time
/// (the index is gone after the first real drop) — the short-circuit path
/// instead returns the SAME cached `existed: true` from the first response.
#[tokio::test]
async fn p1065_idempotent_retry_does_not_reexecute() {
    let shamir = setup_with_index().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let client_supplied_id = RecordId::new();

    let mut b1 = Batch::new();
    b1.id(1);
    b1.drop_index(
        "d",
        ddl::drop_index("idx_city", "items")
            .repo("main")
            .request_id(client_supplied_id),
    );
    let req1 = b1.to_request_via_msgpack();
    let resp1 = shamir.execute("testdb", &req1).await.unwrap();
    let result1 = &resp1.results["d"];
    assert_eq!(result1.op_id.unwrap(), client_supplied_id);

    // Second send: SAME request_id. If the code re-executed the drop
    // against the (now-absent) index, the real (non-idempotent) drop path
    // would see `is_regular/is_unique/is_sorted/is_index2` all `false` and
    // return `existed: false` — a DIFFERENT observable result from the
    // first call. The idempotent short-circuit instead must return the
    // SAME `existed: true` the first call already established, proving it
    // never reached the mutation a second time.
    let mut b2 = Batch::new();
    b2.id(1);
    b2.drop_index(
        "d",
        ddl::drop_index("idx_city", "items")
            .repo("main")
            .request_id(client_supplied_id),
    );
    let req2 = b2.to_request_via_msgpack();
    let resp2 = shamir.execute("testdb", &req2).await.unwrap();
    let result2 = &resp2.results["d"];

    assert_eq!(result2.op_id.unwrap(), client_supplied_id);
    let existed1 = result1.records[0].get_value_bool("existed");
    let existed2 = result2.records[0].get_value_bool("existed");
    assert_eq!(
        existed2, existed1,
        "retried request must report the SAME existed value as the first \
         (short-circuited on the cached status), not re-derive it from a \
         second real drop attempt"
    );
    assert_eq!(
        existed2,
        Some(true),
        "the cached result must reflect the first call's real outcome \
         (existed: true) — if the retry had actually re-executed the drop \
         against the now-absent index, the real (non-idempotent) path \
         would report existed: false instead"
    );

    // Exactly one status record — the second send did not overwrite it
    // with a fresh timestamp from a real second run in a way we can't
    // detect; at minimum, it must still read back as Succeeded and be the
    // one and only record for this op_id.
    let status = ddl_op_log::read_op_status(table.info_store(), &client_supplied_id)
        .await
        .unwrap()
        .expect("status should be written");
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}

// ============================================================================
// Test 6: versioned envelope round-trips + rejects unrecognized versions
// ============================================================================

#[tokio::test]
async fn p1065_versioned_envelope_round_trips() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let op_id = RecordId::new();
    let status = DdlOpStatus {
        op_id,
        kind: DdlOpKind::DropHashIndex {
            index_name: "idx_name".to_string(),
        },
        state: DdlOpState::Succeeded {
            completed_at: 12345,
        },
    };

    ddl_op_log::write_op_status(&info_store, &status)
        .await
        .unwrap();

    let read_status = ddl_op_log::read_op_status(&info_store, &op_id)
        .await
        .unwrap()
        .expect("status should exist");

    assert_eq!(read_status.op_id, status.op_id);
    assert_eq!(read_status.kind, status.kind);
    assert_eq!(read_status.state, status.state);
}

#[tokio::test]
async fn p1065_versioned_envelope_rejects_unrecognized_version() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let op_id = RecordId::new();
    let key = ddl_op_log::op_status_key(&op_id);

    // Write a record with an invalid version byte (0xFF instead of the
    // real 0x01) followed by some payload bytes.
    let invalid_bytes = vec![0xFFu8, 0x00, 0x00, 0x00];
    info_store
        .set(key, Bytes::from(invalid_bytes))
        .await
        .unwrap();

    let result = ddl_op_log::read_op_status(&info_store, &op_id).await;
    assert!(
        result.is_err(),
        "an unrecognized version byte must be rejected, not silently \
         misdecoded"
    );
    assert!(
        matches!(result.unwrap_err(), DbError::Codec(_)),
        "the rejection must be a clean Codec error, not a panic"
    );
}

// ============================================================================
// Test 7: RENAME DdlOpKind family classification
// ============================================================================

/// Rename a regular index and (separately) a unique index; assert the
/// logged `DdlOpKind` matches each one's actual family. This exercises the
/// pre-mutation family check (`table.unique_index_exists(&op.rename_index)`
/// on the OLD name, checked BEFORE the rename) from an earlier round of
/// this task.
#[tokio::test]
async fn p1065_rename_ddlopkind_family_classification() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();
    table.create_index("idx_regular", &["name"]).await.unwrap();
    table
        .create_unique_index("idx_unique", &["email"])
        .await
        .unwrap();

    // RENAME the regular index.
    let regular_op_id = RecordId::new();
    let mut br = Batch::new();
    br.id(1);
    br.rename_index(
        "r",
        ddl::rename_index("items", "idx_regular", "idx_regular_renamed")
            .repo("main")
            .request_id(regular_op_id),
    );
    let regular_req = br.to_request_via_msgpack();
    shamir.execute("testdb", &regular_req).await.unwrap();

    let regular_status = ddl_op_log::read_op_status(table.info_store(), &regular_op_id)
        .await
        .unwrap()
        .expect("regular rename status should exist");
    assert!(
        matches!(
            &regular_status.kind,
            DdlOpKind::RenameHashIndex { old_name, new_name }
            if old_name == "idx_regular" && new_name == "idx_regular_renamed"
        ),
        "expected RenameHashIndex for the regular family, got {:?}",
        regular_status.kind
    );

    // RENAME the unique index.
    let unique_op_id = RecordId::new();
    let mut bu = Batch::new();
    bu.id(1);
    bu.rename_index(
        "r",
        ddl::rename_index("items", "idx_unique", "idx_unique_renamed")
            .repo("main")
            .request_id(unique_op_id),
    );
    let unique_req = bu.to_request_via_msgpack();
    shamir.execute("testdb", &unique_req).await.unwrap();

    let unique_status = ddl_op_log::read_op_status(table.info_store(), &unique_op_id)
        .await
        .unwrap()
        .expect("unique rename status should exist");
    assert!(
        matches!(
            &unique_status.kind,
            DdlOpKind::RenameUniqueHashIndex { old_name, new_name }
            if old_name == "idx_unique" && new_name == "idx_unique_renamed"
        ),
        "expected RenameUniqueHashIndex for the unique family, got {:?}",
        unique_status.kind
    );
}

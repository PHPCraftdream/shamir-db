//! Review finding 3: ordinary validator registration replace and rename
//! take the target admission of every database that binds the validator,
//! with the same shared reservation and stable refusal as bind/unbind,
//! while unbound validators stay unfenced. Also covers the database-rename
//! re-key of the validator `bound_in` tracking the admission discovery
//! reads.

use std::{future::Future, path::Path, time::Duration};

use base64::Engine as _;
use shamir_query_builder::{batch::Batch, ddl};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::WriteOp;
use shamir_storage::types::Repo as _;
use shamir_test_diag::FlakeGuard;
use shamir_types::types::record_id::RecordId;

use super::helpers::{init_waiting_for_locks, on_big_stack, persistent_db, run_admin_op};
use crate::engine::repo::{repo_types::BoxRepoFactory, RepoConfig};
use crate::engine::table::table_store_names;
use crate::query::batch::BatchError;
use crate::storage::storage_membuffer::MemBufferConfig;
use crate::types::value::QueryValue;
use crate::ShamirDb;

const SERVING: &str = "serving";
const TARGET: &str = "target";
const ALT: &str = "alt";
const ALT2: &str = "alt2";
const REPO: &str = "main";
const TABLE: &str = "sentinel";
const BOUND: &str = "bound_guard";
const ALTBOUND: &str = "alt_bound_guard";
const FREE: &str = "free_guard";
const FREE2: &str = "free_guard2";
const HMAC: &str = "test-hmac";
const IN_PROGRESS: &str = "schema_migration_in_progress";
const IN_DOUBT: &str = "schema_migration_in_doubt";
const DEADLINE: Duration = Duration::from_secs(180);

/// Cancel-safe: NO — on timeout the test panics; it does not reuse the engine.
async fn bounded<T>(step: impl Future<Output = T>) -> T {
    tokio::time::timeout(DEADLINE, Box::pin(step))
        .await
        .expect("validator target-fence step timed out")
}

async fn admin(db: &ShamirDb, serving: &str, op: BatchOp) -> QueryValue {
    bounded(run_admin_op(db, serving, "admin", op))
        .await
        .expect("admin op must succeed")
}

async fn refused(db: &ShamirDb, serving: &str, op: BatchOp, code: &str, case: &str) {
    let error: BatchError = bounded(run_admin_op(db, serving, "admin", op))
        .await
        .expect_err(&format!("{case}: unexpectedly admitted"));
    assert_eq!(error.code(), Some(code), "{case}: {error}");
}

fn wasm(memory_pages: u32) -> String {
    // The same minimal uploaded-module ABI as the validator admission
    // fixture; the page count distinguishes replacement artifacts so a
    // successful replace is observable through the persisted module hash.
    base64::engine::general_purpose::STANDARD.encode(
        wat::parse_str(format!(
            r#"(module
        (memory (export "memory") {memory_pages})
        (func (export "shamir_alloc") (param i32) (result i32) i32.const 1024)
        (func (export "shamir_call") (param i32 i32) (result i64) i64.const 0)
    )"#
        ))
        .unwrap(),
    )
}

fn create_validator(name: &str) -> BatchOp {
    ddl::create_validator(name).wasm(wasm(2)).build().unwrap()
}

fn replace_validator(name: &str) -> BatchOp {
    replace_validator_with(name, 3)
}

fn replace_validator_with(name: &str, pages: u32) -> BatchOp {
    ddl::create_validator(name)
        .wasm(wasm(pages))
        .replace()
        .hmac(HMAC)
        .build()
        .unwrap()
}

async fn module_hash(db: &ShamirDb, name: &str) -> String {
    bounded(db.system_store().load_validator(name))
        .await
        .unwrap()
        .expect("validator row")
        .get("module_hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn rename_validator(from: &str, to: &str) -> BatchOp {
    ddl::rename_validator(from, to)
}

async fn create_with_table(db: &ShamirDb, root: &Path, name: &str, suffix: &str) {
    bounded(db.create_db(name)).await.unwrap();
    let path = root.join(format!("{name}-{suffix}"));
    bounded(db.add_repo(name, RepoConfig::new(REPO, BoxRepoFactory::fjall(&path))))
        .await
        .unwrap();
    let mut batch = Batch::new();
    batch.op("table", ddl::create_table(TABLE).repo(REPO).build());
    let request = batch.to_request_via_msgpack().unwrap();
    bounded(db.execute(name, &request)).await.unwrap();
}

/// Buffered in-memory fixture; the background flusher may drain at any time,
/// so the tests rely on the bind/unbind flush, never on a drain happening.
async fn create_with_buffered_table(db: &ShamirDb, name: &str) {
    bounded(db.create_db(name)).await.unwrap();
    bounded(db.add_repo(
        name,
        RepoConfig::new(
            REPO,
            BoxRepoFactory::membuffer(
                BoxRepoFactory::in_memory(),
                MemBufferConfig {
                    max_bytes: 1_000_000,
                    max_entries: 10_000,
                    ttl_ms: None,
                    flush_interval_ms: 60_000,
                    flush_batch_size: 256,
                },
            ),
        ),
    ))
    .await
    .unwrap();
    let mut batch = Batch::new();
    batch.op("table", ddl::create_table(TABLE).repo(REPO).build());
    let request = batch.to_request_via_msgpack().unwrap();
    bounded(db.execute(name, &request)).await.unwrap();
}

async fn bind(db: &ShamirDb, target: &str, validator: &str) {
    admin(
        db,
        SERVING,
        ddl::bind_validator(validator, TABLE)
            .db(target)
            .repo(REPO)
            .ops([WriteOp::Insert])
            .build(),
    )
    .await;
}

async fn assert_bound_tables(db: &ShamirDb, id: RecordId, mut expected: Vec<String>) {
    expected.sort();
    let mut actual = db.validators().bound_tables(&id);
    actual.sort();
    assert_eq!(actual, expected, "bound table refs");
}

#[test]
fn bound_validator_registration_refuses_fenced_binding_holders() {
    on_big_stack(|| async {
        let watchdog = FlakeGuard::with_watchdog(
            "bound_validator_registration_refuses_fenced_binding_holders",
            Duration::from_secs(600),
        );
        let (dir, db) = bounded(persistent_db()).await;
        create_with_table(&db, dir.path(), SERVING, "fixture").await;
        create_with_table(&db, dir.path(), TARGET, "fixture").await;
        create_with_table(&db, dir.path(), ALT, "fixture").await;

        watchdog.mark("fixture validators and bindings");
        admin(&db, SERVING, create_validator(BOUND)).await;
        admin(&db, SERVING, create_validator(ALTBOUND)).await;
        admin(&db, SERVING, create_validator(FREE)).await;
        let bound_id = db.validators().id_for_name(BOUND).unwrap();
        let altbound_id = db.validators().id_for_name(ALTBOUND).unwrap();
        let free_id = db.validators().id_for_name(FREE).unwrap();
        bind(&db, TARGET, BOUND).await;
        bind(&db, ALT, BOUND).await;
        bind(&db, ALT, ALTBOUND).await;
        assert_bound_tables(
            &db,
            bound_id,
            vec![
                format!("{ALT}/{REPO}/{TABLE}"),
                format!("{TARGET}/{REPO}/{TABLE}"),
            ],
        )
        .await;

        watchdog.mark("reserved fence refuses replace and rename");
        let mut gate = db.reserve_schema_migration_gate(TARGET).unwrap();
        let bound_hash_before = module_hash(&db, BOUND).await;
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_PROGRESS,
            "replace under reserved fence",
        )
        .await;
        assert_eq!(db.validators().id_for_name(BOUND), Some(bound_id));
        assert_bound_tables(
            &db,
            bound_id,
            vec![
                format!("{ALT}/{REPO}/{TABLE}"),
                format!("{TARGET}/{REPO}/{TABLE}"),
            ],
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(BOUND, "moved"),
            IN_PROGRESS,
            "rename under reserved fence",
        )
        .await;
        assert_eq!(db.validators().id_for_name(BOUND), Some(bound_id));
        assert_eq!(
            module_hash(&db, BOUND).await,
            bound_hash_before,
            "a refused replace must not swap the artifact"
        );

        watchdog.mark("unfenced validators stay unfenced");
        let free_hash_before = module_hash(&db, FREE).await;
        let replaced = admin(&db, SERVING, replace_validator(FREE)).await;
        assert_eq!(replaced["created_validator"].as_str(), Some(FREE));
        assert_eq!(db.validators().id_for_name(FREE), Some(free_id));
        assert_ne!(
            module_hash(&db, FREE).await,
            free_hash_before,
            "the replacement artifact must actually swap the module"
        );
        let renamed = admin(&db, SERVING, rename_validator(FREE, FREE2)).await;
        assert_eq!(renamed["renamed_validator"].as_str(), Some(FREE));
        assert_eq!(db.validators().id_for_name(FREE), None);
        assert_eq!(db.validators().id_for_name(FREE2), Some(free_id));
        let row = bounded(db.system_store().load_validator(FREE2))
            .await
            .unwrap()
            .expect("renamed validator keeps a durable row");
        assert_eq!(row["_id"].as_str(), Some(free_id.to_string().as_str()));
        assert!(
            bounded(db.system_store().load_validator(FREE))
                .await
                .unwrap()
                .is_none(),
            "old durable row must be re-keyed"
        );

        watchdog.mark("in-doubt fence refuses replace and rename");
        gate.hold(1);
        drop(gate);
        assert_eq!(
            db.schema_migration_gate_in_doubt_generation(TARGET),
            Some(1)
        );
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_DOUBT,
            "replace under in-doubt fence",
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(BOUND, "moved"),
            IN_DOUBT,
            "rename under in-doubt fence",
        )
        .await;

        watchdog.mark("database rename re-keys the bound tracking");
        admin(&db, SERVING, ddl::rename_db(ALT, ALT2).build()).await;
        assert_eq!(db.validators().id_for_name(BOUND), Some(bound_id));
        assert_bound_tables(
            &db,
            bound_id,
            vec![
                format!("{ALT2}/{REPO}/{TABLE}"),
                format!("{TARGET}/{REPO}/{TABLE}"),
            ],
        )
        .await;
        let row = bounded(db.system_store().load_validator(BOUND))
            .await
            .unwrap()
            .expect("bound validator keeps its durable row");
        let bound_in: Vec<String> = row["bound_in"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect();
        assert!(bound_in.contains(&format!("{ALT2}/{REPO}/{TABLE}")));
        assert!(!bound_in.contains(&format!("{ALT}/{REPO}/{TABLE}")));

        watchdog.mark("replace with only free bindings succeeds and keeps them");
        let altbound_hash_before = module_hash(&db, ALTBOUND).await;
        let replaced = admin(&db, SERVING, replace_validator(ALTBOUND)).await;
        assert_eq!(replaced["created_validator"].as_str(), Some(ALTBOUND));
        assert_eq!(db.validators().id_for_name(ALTBOUND), Some(altbound_id));
        assert_ne!(
            module_hash(&db, ALTBOUND).await,
            altbound_hash_before,
            "the free-only replace must actually swap the module"
        );
        assert_bound_tables(&db, altbound_id, vec![format!("{ALT2}/{REPO}/{TABLE}")]).await;

        watchdog.mark("admission follows the re-keyed name");
        let mut gate2 = db.reserve_schema_migration_gate(ALT2).unwrap();
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_PROGRESS,
            "replace after rename, reserved",
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(ALTBOUND, "moved"),
            IN_PROGRESS,
            "rename after rename, reserved",
        )
        .await;
        gate2.hold(2);
        drop(gate2);
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_DOUBT,
            "replace after rename, in doubt",
        )
        .await;
        assert_eq!(db.validators().id_for_name(BOUND), Some(bound_id));
        assert_eq!(
            module_hash(&db, BOUND).await,
            bound_hash_before,
            "the in-doubt refusal must not swap the artifact"
        );
    });
}
// Round 4 items 6(b), 3, and the buffered binding-scope regressions.

const OLD: &str = "olddb";
const NEW: &str = "newdb";
const REPO2: &str = "main2";
const TABLE2: &str = "sentinel2";
const SETTINGS_TABLE: &str = "settings";
const REAL: &str = "real_bound_guard";
const STALE: &str = "stale_guard";
const GHOST_REF: &str = "ghost/nonexistent/sentinel";
const VALIDATORS_TABLE: &str = "validators";

fn drop_validator_op(name: &str) -> BatchOp {
    ddl::drop_validator(name).hmac(HMAC).build().unwrap()
}

fn unbind_op(db_name: &str, repo: &str, table: &str, validator: &str) -> BatchOp {
    ddl::unbind_validator(validator, table)
        .db(db_name)
        .repo(repo)
        .build()
}

/// The table must hold the real binding — the premise every admission
/// derivation below has to recover.
async fn assert_table_binds(
    db: &ShamirDb,
    database: &str,
    repo: &str,
    table: &str,
    id: RecordId,
    case: &str,
) {
    let bindings = bounded(db.list_validator_bindings(database, repo, table))
        .await
        .expect("table bindings are readable");
    assert!(
        bindings.iter().any(|binding| binding.validator_id == id),
        "{case}: the table must hold the real binding"
    );
}

/// WAL-backed merge-patch of one validator catalogue row's `bound_in`
/// (unique `name` key) through the system repo's implicit batch
/// transaction — one durable WAL entry, so the crafted state replays on
/// reopen. Same seam as the journal fixtures' row patches.
async fn patch_bound_in(db: &ShamirDb, name: &str, refs: &[&str]) {
    let table = db
        .system_store()
        .system_repo()
        .expect("system repo")
        .get_table(VALIDATORS_TABLE)
        .await
        .expect("validators catalogue table");
    let list = QueryValue::List(
        refs.iter()
            .map(|reference| QueryValue::Str((*reference).to_string()))
            .collect(),
    );
    let mut patch = shamir_types::types::common::new_map();
    patch.insert("bound_in".to_string(), list);
    let op = shamir_query_builder::write::update(VALIDATORS_TABLE)
        .where_(shamir_query_builder::filter::eq("name", name))
        .set(QueryValue::Map(patch))
        .build()
        .expect("typed validators row update");
    let result = db
        .system_store()
        .update_via_implicit_tx(&table, &op)
        .await
        .expect("WAL-backed validators row update commits");
    assert_eq!(
        result.affected, 1,
        "the bound_in patch must hit exactly the '{name}' row"
    );
    db.system_store()
        .flush_system_repo_durable(&table)
        .await
        .expect("the patched validators row flushes durably");
}

/// Drop the engine and reopen the SAME persistent store. The directory
/// binding stays alive for the whole test; the reopen retries only the
/// Windows `Locked` diagnostic under a bounded deadline (shared helpers).
async fn reopen(dir: &Path, db: ShamirDb) -> ShamirDb {
    drop(db);
    init_waiting_for_locks(dir.join("system")).await
}

async fn validator_row(db: &ShamirDb, name: &str) -> Option<QueryValue> {
    bounded(db.system_store().load_validator(name))
        .await
        .unwrap()
}

async fn bound_in_refs(db: &ShamirDb, name: &str) -> Vec<String> {
    validator_row(db, name)
        .await
        .expect("validator row exists")
        .get("bound_in")
        .and_then(|value| value.as_array())
        .map(|array| {
            array
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// DROP's refusal is two-layered on the wire: the admitted drop reserves
/// the holders' fences FIRST, so a FENCED holder surfaces its exact stable
/// refusal code; an UNFENCED but still-bound validator refuses with the
/// still-bound message naming the authoritative holder ref.
async fn refused_still_bound(
    db: &ShamirDb,
    serving: &str,
    op: BatchOp,
    holder_ref: &str,
    case: &str,
) {
    let error: BatchError = bounded(run_admin_op(db, serving, "admin", op))
        .await
        .expect_err(&format!("{case}: unexpectedly admitted"));
    let message = error.to_string();
    assert!(
        message.contains("still bound"),
        "{case}: expected the still-bound refusal, got {message}"
    );
    assert!(
        message.contains(holder_ref),
        "{case}: the refusal must name the authoritative holder \
         '{holder_ref}', got {message}"
    );
}

/// Item 1(a): bind in TARGET while the binding's own reverse save fails for
/// real (the swallowed best-effort `bound_in` write), so the catalogue
/// record keeps NO reverse reference at all — the omitted case. After a
/// clean restart the registry restores no TARGET reference, and the fenced
/// TARGET must still refuse replace, rename and drop served by SERVING:
/// admission has to recover the holder from the table's own binding.
/// Without authoritative derivation the replace swaps (and the drop
/// removes) the very artifact TARGET's table still invokes.
#[test]
fn bound_validator_admission_survives_a_missing_reverse_row() {
    on_big_stack(|| async {
        let watchdog = FlakeGuard::with_watchdog(
            "bound_validator_admission_survives_a_missing_reverse_row",
            Duration::from_secs(600),
        );
        let (dir, db) = bounded(persistent_db()).await;
        create_with_table(&db, dir.path(), SERVING, "fixture").await;
        create_with_table(&db, dir.path(), TARGET, "fixture").await;

        watchdog.mark("bind while the reverse save fails for real");
        admin(&db, SERVING, create_validator(BOUND)).await;
        let bound_id = db.validators().id_for_name(BOUND).unwrap();
        db.system_store().arm_catalogue_write_fault(1);
        bind(&db, TARGET, BOUND).await;
        assert_table_binds(&db, TARGET, REPO, TABLE, bound_id, "before restart").await;
        assert!(
            !bound_in_refs(&db, BOUND)
                .await
                .iter()
                .any(|reference| reference.starts_with(TARGET)),
            "the reverse row must be missing the TARGET reference"
        );

        watchdog.mark("restart");
        let db = reopen(dir.path(), db).await;
        assert_eq!(
            db.validators().id_for_name(BOUND).unwrap(),
            bound_id,
            "the validator identity is restored from the catalogue"
        );
        assert_table_binds(&db, TARGET, REPO, TABLE, bound_id, "after restart").await;

        watchdog.mark("fenced TARGET refuses despite the omitted reverse row");
        let gate = db.reserve_schema_migration_gate(TARGET).unwrap();
        let hash_before = module_hash(&db, BOUND).await;
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_PROGRESS,
            "replace with an omitted reverse row",
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(BOUND, "moved"),
            IN_PROGRESS,
            "rename with an omitted reverse row",
        )
        .await;
        refused(
            &db,
            SERVING,
            drop_validator_op(BOUND),
            IN_PROGRESS,
            "drop under the fenced TARGET holder",
        )
        .await;
        assert_eq!(db.validators().id_for_name(BOUND), Some(bound_id));
        assert_eq!(
            module_hash(&db, BOUND).await,
            hash_before,
            "the refused replace must not swap the artifact"
        );
        drop(gate);

        // Still bound here (the unbind is below): the scan must refuse.
        refused_still_bound(
            &db,
            SERVING,
            drop_validator_op(BOUND),
            &format!("{TARGET}/{REPO}/{TABLE}"),
            "unfenced drop of the bound validator",
        )
        .await;

        watchdog.mark("after a real unbind the drop goes through");
        // The unbind flushes the info record, so the later scan sees it unbound.
        admin(&db, SERVING, unbind_op(TARGET, REPO, TABLE, BOUND)).await;
        admin(&db, SERVING, drop_validator_op(BOUND)).await;
        assert_eq!(db.validators().id_for_name(BOUND), None);
        assert!(
            validator_row(&db, BOUND).await.is_none(),
            "the dropped validator's durable row must be gone"
        );
    });
}

/// Item 1(b): the database rename re-key moves the namespace rows first and
/// saves the validator's `bound_in` row last. Failing that LAST write for
/// real (the 7th gated catalogue write of the rename: database save+remove,
/// repository save+remove, table save+remove, validator save) leaves the
/// namespace under NEW with the validator row still naming OLD. After the
/// restart the recreated free OLD incarnation must NOT be the reserved
/// identity — the actual binding lives in NEW's moved table, and only NEW's
/// fence refuses, naming NEW's CURRENT ref despite the stale row. The
/// unbind then runs at the CURRENT names and the drop goes through with the
/// stale child names still in the reverse row.
#[test]
fn rename_rekey_fault_reserves_the_new_identity_alone() {
    on_big_stack(|| async {
        let watchdog = FlakeGuard::with_watchdog(
            "rename_rekey_fault_reserves_the_new_identity_alone",
            Duration::from_secs(600),
        );
        let (dir, db) = bounded(persistent_db()).await;
        create_with_table(&db, dir.path(), SERVING, "fixture").await;
        create_with_table(&db, dir.path(), OLD, "fixture").await;

        watchdog.mark("bind, then rename repo and table under OLD");
        admin(&db, SERVING, create_validator(BOUND)).await;
        let bound_id = db.validators().id_for_name(BOUND).unwrap();
        let hash_before = module_hash(&db, BOUND).await;
        bind(&db, OLD, BOUND).await;
        admin(&db, OLD, ddl::rename_repo(REPO, REPO2).build()).await;
        admin(
            &db,
            OLD,
            ddl::rename_table(TABLE, TABLE2).repo(REPO2).build(),
        )
        .await;
        assert!(
            bound_in_refs(&db, BOUND)
                .await
                .contains(&format!("{OLD}/{REPO}/{TABLE}")),
            "the child renames leave the stale child reference"
        );

        watchdog.mark("fail the 7th gated catalogue write: the validator row re-key");
        db.system_store().arm_catalogue_write_fault(7);
        let error: BatchError = bounded(run_admin_op(
            &db,
            SERVING,
            "admin",
            ddl::rename_db(OLD, NEW).build(),
        ))
        .await
        .expect_err("the faulted rename must surface its catalogue failure");
        assert!(
            error
                .to_string()
                .contains("injected catalogue write failure"),
            "the 7th-write fault must surface itself, got {error}"
        );
        // The namespace rows DID move (write-before-remove per child).
        let repos = bounded(db.system_store().load_repositories())
            .await
            .unwrap();
        assert!(
            repos
                .iter()
                .any(|record| record.get("db_name").and_then(QueryValue::as_str) == Some(NEW)),
            "the repository row must have moved to NEW"
        );
        assert_eq!(
            bound_in_refs(&db, BOUND).await,
            vec![format!("{OLD}/{REPO}/{TABLE}")],
            "the validator row must still name OLD"
        );

        watchdog.mark("restart and recreate a free OLD at a DISTINCT repo path");
        let db = reopen(dir.path(), db).await;
        assert_eq!(db.validators().id_for_name(BOUND).unwrap(), bound_id);
        create_with_table(&db, dir.path(), OLD, "fixture-recreated").await;
        assert_table_binds(&db, NEW, REPO2, TABLE2, bound_id, "NEW holds the binding").await;

        watchdog.mark("fenced NEW refuses, naming its CURRENT ref");
        let gate = db.reserve_schema_migration_gate(NEW).unwrap();
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_PROGRESS,
            "replace with the stale row pointing at OLD",
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(BOUND, "moved"),
            IN_PROGRESS,
            "rename with the stale row pointing at OLD",
        )
        .await;
        refused(
            &db,
            SERVING,
            drop_validator_op(BOUND),
            IN_PROGRESS,
            "drop under the fenced NEW holder",
        )
        .await;
        assert_eq!(
            module_hash(&db, BOUND).await,
            hash_before,
            "no artifact swap under the refused admission"
        );
        drop(gate);

        watchdog.mark("unfenced: still-bound refusal names the CURRENT NEW ref");
        refused_still_bound(
            &db,
            SERVING,
            drop_validator_op(BOUND),
            &format!("{NEW}/{REPO2}/{TABLE2}"),
            "unfenced drop with the stale row pointing at OLD",
        )
        .await;

        watchdog.mark("the recreated OLD incarnation is NOT the reserved identity");
        let gate = db.reserve_schema_migration_gate(OLD).unwrap();
        let replaced = admin(&db, SERVING, replace_validator(BOUND)).await;
        assert_eq!(replaced["created_validator"].as_str(), Some(BOUND));
        assert_eq!(
            db.validators().id_for_name(BOUND),
            Some(bound_id),
            "the replace dedupes to the same identity"
        );
        assert_ne!(
            module_hash(&db, BOUND).await,
            hash_before,
            "the admitted replace must actually swap the module"
        );
        drop(gate);

        watchdog.mark("unbind at the CURRENT names; drop despite stale child names");
        admin(&db, NEW, unbind_op(NEW, REPO2, TABLE2, BOUND)).await;
        assert!(
            bound_in_refs(&db, BOUND)
                .await
                .contains(&format!("{OLD}/{REPO}/{TABLE}")),
            "precondition: the stale child name is still in the reverse row"
        );
        admin(&db, SERVING, drop_validator_op(BOUND)).await;
        assert_eq!(db.validators().id_for_name(BOUND), None);
        assert!(
            validator_row(&db, BOUND).await.is_none(),
            "the drop must not be blocked by the stale reverse row"
        );
    });
}

/// Item 1(c): a reverse row naming a NONEXISTENT database is state no
/// production writer can produce, so it is crafted through the durable
/// catalogue (WAL-backed patch, restored by the restart). On the actually
/// bound REAL the stale row REPLACES the real reference: TARGET's fence
/// must still refuse (a stale row must not admit a bound one, and the
/// missing real reference must not drop the holder). On the genuinely
/// unbound STALE the same ghost reference must neither block its replace
/// nor keep its drop refused (a stale row must not falsely bind).
#[test]
fn stale_reverse_rows_neither_block_a_replace_nor_admit_a_bound_one() {
    on_big_stack(|| async {
        let watchdog = FlakeGuard::with_watchdog(
            "stale_reverse_rows_neither_block_a_replace_nor_admit_a_bound_one",
            Duration::from_secs(600),
        );
        let (dir, db) = bounded(persistent_db()).await;
        create_with_table(&db, dir.path(), SERVING, "fixture").await;
        create_with_table(&db, dir.path(), TARGET, "fixture").await;

        watchdog.mark("fixture validators, one real binding, two ghost rows");
        admin(&db, SERVING, create_validator(REAL)).await;
        admin(&db, SERVING, create_validator(STALE)).await;
        let real_id = db.validators().id_for_name(REAL).unwrap();
        bind(&db, TARGET, REAL).await;
        patch_bound_in(&db, REAL, &[GHOST_REF]).await;
        patch_bound_in(&db, STALE, &[GHOST_REF]).await;

        watchdog.mark("restart");
        let db = reopen(dir.path(), db).await;
        assert_eq!(db.validators().id_for_name(REAL).unwrap(), real_id);

        watchdog.mark("TARGET's fence still refuses the stale-only bound row");
        let gate = db.reserve_schema_migration_gate(TARGET).unwrap();
        refused(
            &db,
            SERVING,
            replace_validator(REAL),
            IN_PROGRESS,
            "replace of the bound validator with a ghost-only row",
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(REAL, "moved"),
            IN_PROGRESS,
            "rename of the bound validator with a ghost-only row",
        )
        .await;
        refused(
            &db,
            SERVING,
            drop_validator_op(REAL),
            IN_PROGRESS,
            "drop under the fenced TARGET holder",
        )
        .await;
        drop(gate);

        watchdog.mark("unfenced: still-bound refusal names the authoritative ref");
        refused_still_bound(
            &db,
            SERVING,
            drop_validator_op(REAL),
            &format!("{TARGET}/{REPO}/{TABLE}"),
            "unfenced drop with a ghost-only row",
        )
        .await;

        watchdog.mark("the ghost row neither blocks nor binds the free validator");
        let replaced = admin(&db, SERVING, replace_validator(STALE)).await;
        assert_eq!(replaced["created_validator"].as_str(), Some(STALE));
        admin(&db, SERVING, drop_validator_op(STALE)).await;
        assert_eq!(db.validators().id_for_name(STALE), None);
        assert!(
            validator_row(&db, STALE).await.is_none(),
            "the stale unbound row must be droppable"
        );

        watchdog.mark("the real binding is removable and then droppable");
        admin(&db, SERVING, unbind_op(TARGET, REPO, TABLE, REAL)).await;
        admin(&db, SERVING, drop_validator_op(REAL)).await;
        assert_eq!(db.validators().id_for_name(REAL), None);
        assert!(validator_row(&db, REAL).await.is_none());
    });
}

/// Item 6(b): an acknowledged bind/unbind remains visible to an independent
/// reverse scan even when the user repository uses a write-back buffer. The
/// replacement, rename and DROP refusals must be target-fence refusals, while
/// unbind before DROP must not be mistaken for a surviving stale envelope.
#[test]
fn buffered_bindings_are_visible_to_foreign_validator_admission() {
    on_big_stack(|| async {
        let watchdog = FlakeGuard::with_watchdog(
            "buffered_bindings_are_visible_to_foreign_validator_admission",
            Duration::from_secs(600),
        );
        let (dir, db) = bounded(persistent_db()).await;
        create_with_table(&db, dir.path(), SERVING, "fixture").await;
        let mut lazy = Batch::new();
        lazy.op(
            "lazy",
            ddl::create_table("lazy_untouched").repo(REPO).build(),
        );
        bounded(db.execute(SERVING, &lazy.to_request_via_msgpack().unwrap()))
            .await
            .unwrap();
        let serving = db.get_db(SERVING).unwrap();
        let serving_repo = serving.get_repo(REPO).unwrap();
        let backing = serving_repo.seed_backing_repo();
        let lazy_config = serving_repo.table_config("lazy_untouched").unwrap();
        let lazy_binding_name =
            table_store_names("lazy_untouched", lazy_config.store_id.as_ref()).info;
        assert!(
            !backing
                .stores_list()
                .await
                .unwrap()
                .contains(&lazy_binding_name),
            "metadata scan must not create the earlier lazy table's info store"
        );

        create_with_buffered_table(&db, TARGET).await;
        let target_repo = db.get_db(TARGET).unwrap().get_repo(REPO).unwrap();
        let target_backing = target_repo.seed_backing_repo();
        let target_info = target_repo
            .table_config(TABLE)
            .map(|config| table_store_names(TABLE, config.store_id.as_ref()).info)
            .unwrap();

        admin(&db, SERVING, create_validator(BOUND)).await;
        let bound_id = db.validators().id_for_name(BOUND).unwrap();
        bind(&db, TARGET, BOUND).await;
        let mut gate = db.reserve_schema_migration_gate(TARGET).unwrap();
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_PROGRESS,
            "buffered replace",
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(BOUND, "buffered_moved"),
            IN_PROGRESS,
            "buffered rename",
        )
        .await;
        refused(
            &db,
            SERVING,
            drop_validator_op(BOUND),
            IN_PROGRESS,
            "buffered drop",
        )
        .await;
        let store_names_after_scan = target_backing.stores_list().await.unwrap();
        assert!(store_names_after_scan.contains(&target_info));
        assert!(!backing
            .stores_list()
            .await
            .unwrap()
            .contains(&lazy_binding_name));
        assert_eq!(
            db.validators().id_for_name(BOUND),
            Some(bound_id),
            "authoritative holder scan refused before mutating the validator"
        );
        gate.mark_succeeded();
        drop(gate);

        watchdog.mark("rename TABLE while free, then fence the renamed binding");
        admin(
            &db,
            TARGET,
            ddl::rename_table(TABLE, TABLE2).repo(REPO).build(),
        )
        .await;
        let renamed_info = target_repo
            .table_config(TABLE2)
            .map(|config| table_store_names(TABLE2, config.store_id.as_ref()).info)
            .unwrap();
        assert!(
            target_backing
                .stores_list()
                .await
                .unwrap()
                .contains(&renamed_info),
            "rename must flush the destination metadata envelope before unbind"
        );

        let mut renamed_gate = db.reserve_schema_migration_gate(TARGET).unwrap();
        refused(
            &db,
            SERVING,
            replace_validator(BOUND),
            IN_PROGRESS,
            "renamed-table replace",
        )
        .await;
        refused(
            &db,
            SERVING,
            rename_validator(BOUND, "buffered_moved"),
            IN_PROGRESS,
            "renamed-table rename",
        )
        .await;
        refused(
            &db,
            SERVING,
            drop_validator_op(BOUND),
            IN_PROGRESS,
            "renamed-table drop",
        )
        .await;
        renamed_gate.mark_succeeded();
        drop(renamed_gate);

        watchdog.mark("unbind the renamed table without a manual drain");
        admin(&db, SERVING, unbind_op(TARGET, REPO, TABLE2, BOUND)).await;
        admin(&db, SERVING, drop_validator_op(BOUND)).await;
        assert_eq!(db.validators().id_for_name(BOUND), None);
    });
}

#[path = "internal_system_binding_tests.rs"]
mod internal_system_binding_tests;

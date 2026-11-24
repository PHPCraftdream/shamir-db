//! Cold-resolver cell retention and slot-saturation tests of the point read (a child
//! module of `get_record_authz_tests`: it reuses that file's private helpers).

use super::*;

// ---------------------------------------------------------------------------
// Cold-resolver cell retention on the batch-read and cursor-create paths
// (moved here verbatim from the former `unknown_table_cell_growth_tests`
// module, which the layout folder limit no longer allows as its own file).
// ---------------------------------------------------------------------------

// Regression for cold-resolver cell retention in `RepoCore::get_table_ctx`:
// the table cell used to be inserted BEFORE the configured-table check, so
// every request naming a NEVER-created table left an empty `OnceCell` in
// the repository's `tables` map forever — N distinct unknown names grew it
// without bound.
//
// The engine's own tests cannot observe this (`RepoInstance::table_cell_count`
// is `test-util`-gated and `shamir-engine`'s tests do not enable it), so the
// probe lives here, where the dev-dependency does.
//
// Each subject request must reach the configured-table refusal (the message
// carries the engine's "is not configured in this repository" text) and
// leave the cell map unchanged. WITHOUT the pre-check every probe inserts
// one cell, so the count grows by the number of distinct names.
//
// Fixture mirrors the point-read tests above (in-memory `ShamirDb`, owner
// alice, repo `main` with table `items`; requests are builder-produced and
// dispatched through the real handler; every await is bounded by a 120 s
// timeout because the Windows CI runner may be up to 10x slower). No temp
// dirs and no sleeps are involved: the repository is purely in-memory and
// lives for the whole test.

/// Distinct never-created names per subject (the task's minimum is 8).
const UNKNOWN_NAMES: usize = 8;

/// Handler over an in-memory `ShamirDb` with `app.main.items` owned by alice.
/// `items` is CONFIGURED but never opened, so its own cell is not part of the
/// baseline either.
async fn build_growth_handler() -> ShamirDbHandler {
    let shamir = tokio::time::timeout(CALL_TIMEOUT, ShamirDb::init_memory())
        .await
        .expect("init must complete within the 120s bound")
        .expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    tokio::time::timeout(CALL_TIMEOUT, shamir.create_db_as("app", owner.clone()))
        .await
        .expect("create_db must complete within the 120s bound")
        .expect("create_db");
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    tokio::time::timeout(CALL_TIMEOUT, shamir.add_repo_as("app", cfg, owner))
        .await
        .expect("add repo must complete within the 120s bound")
        .expect("add repo");
    ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(CursorLimitsCap::UNLIMITED)
}

/// Eight DISTINCT never-created table names.
fn unknown_names() -> impl Iterator<Item = String> {
    (0..UNKNOWN_NAMES).map(|i| format!("absent_table_{i}"))
}

/// Live cell-map size of the repository the requests resolve against.
fn table_cell_count(handler: &ShamirDbHandler) -> usize {
    handler
        .db()
        .get_db("app")
        .expect("app database")
        .get_repo("main")
        .expect("main repo")
        .table_cell_count()
}

fn batch_read_req(table: &str) -> DbRequest {
    let mut batch = Batch::new();
    batch.query("read", Query::with_repo("main", table));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        batch: batch.build(),
    }
}

fn create_cursor_req(table: &str) -> DbRequest {
    DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        query: ReadQuery::with_repo("main", table),
        page_size: 1,
    }
}

/// The batch path refuses an unknown table with the canonical, name-hiding
/// `unknown_table` response.
fn expect_batch_unknown_table_refusal(resp: DbResponse, table: &str) {
    match resp {
        DbResponse::Error { code, message } => assert_eq!(
            (code.as_str(), message.as_str()),
            ("unknown_table", "table is unavailable"),
            "batch read of unknown table '{table}'"
        ),
        other => panic!("batch read of unknown table '{table}': got {other:?}"),
    }
}

/// Cursor creation uses the same canonical, name-hiding refusal and never
/// becomes a cursor.
fn expect_cursor_unknown_table_refusal(resp: DbResponse, table: &str) {
    match resp {
        DbResponse::Error { code, message } => assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "unknown_table",
                "Query '' failed [unknown_table]: table is unavailable"
            ),
            "cursor create of unknown table '{table}'"
        ),
        other => panic!("cursor create of unknown table '{table}': got {other:?}"),
    }
}

/// Directly probe the cold resolver (not `GetRecord`, `Execute`, or cursor
/// preflight): two distinct unknown names must not allocate catalogue cells.
#[tokio::test]
async fn cold_resolver_unknown_names_do_not_grow_catalogue_cells() {
    let handler = build_growth_handler().await;
    let before = table_cell_count(&handler);
    let db = handler.db();

    for name in ["cold_absent_one", "cold_absent_two"] {
        assert!(
            db.get_table("app", "main", name).await.is_err(),
            "the cold resolver must refuse unknown table '{name}'"
        );
    }

    assert_eq!(
        table_cell_count(&handler),
        before,
        "direct cold-resolver refusals must not insert catalogue cells"
    );
}

#[tokio::test]
async fn batch_reads_of_unknown_tables_do_not_grow_catalogue_cells() {
    let handler = build_growth_handler().await;
    let session = superuser_alice_session();
    let before = table_cell_count(&handler);

    for name in unknown_names() {
        let resp = send(&handler, &session, batch_read_req(name.as_str())).await;
        expect_batch_unknown_table_refusal(resp, name.as_str());
    }

    assert_eq!(
        table_cell_count(&handler),
        before,
        "a refused batch read must not insert a catalogue cell for its table"
    );
}

/// (b) The same eight names, each a refused cursor create, must leave the
/// cell map unchanged.
///
/// WITHOUT the fix these eight requests grow the cell map by eight.
#[tokio::test]
async fn cursor_creates_over_unknown_tables_do_not_grow_catalogue_cells() {
    let handler = build_growth_handler().await;
    let session = superuser_alice_session();
    let before = table_cell_count(&handler);

    for name in unknown_names() {
        let resp = send(&handler, &session, create_cursor_req(name.as_str())).await;
        expect_cursor_unknown_table_refusal(resp, name.as_str());
    }

    assert_eq!(
        table_cell_count(&handler),
        before,
        "a refused cursor create must not insert a catalogue cell for its table"
    );
}

/// PAIR under a SATURATED request-slot pool: absent and existing-but-closed
/// tables must both answer the fixed refusal, never `server_busy`; availability
/// is decided BEFORE the slot gate, so the absent name cannot reach the
/// exhausted slot while the closed one stops at the table ACL.
#[tokio::test]
async fn get_record_absent_and_closed_tables_are_never_server_busy_under_saturated_slot() {
    let (handler, seeded) = build_handler(ByteBudget::unbounded(), parked_limits()).await;
    let store = SessionStore::new();
    let session = store.insert(ALICE_SID, alice_session());
    chmod_items_table(&handler, 0o000).await;
    let held = session.try_acquire_request_slot(1).expect("the cap-1 slot");
    let closed = send(&handler, &session, get_record_req(seeded)).await;
    let absent = send(
        &handler,
        &session,
        get_record("app", "main", "absent_items", seeded),
    )
    .await;
    drop(held);
    for (resp, label) in [(closed, "closed"), (absent, "absent")] {
        let (code, message) = match resp {
            DbResponse::Error { code, message } => (code, message),
            other => panic!("{label} table: expected the fixed refusal, got {other:?}"),
        };
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("unknown_table", "table is unavailable"),
            "{label} table under a saturated slot"
        );
    }
}

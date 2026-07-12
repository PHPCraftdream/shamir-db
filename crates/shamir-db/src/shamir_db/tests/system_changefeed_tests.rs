//! PR0 smoke-test: system-repo writes flow through the changefeed.
//!
//! Goal (V1a, REPLICATION.md §7.1): prove that administrative mutations on
//! the system store (e.g. `create_user`) are captured by the durable
//! changefeed journal, so account replication can ride the ordinary
//! data-stream pipeline instead of a bespoke path.
//!
//! The system store lives under the reserved db name `__system__`, repo
//! `system`, and is intentionally NOT registered in `ShamirDb::dbs` (see
//! `shamir_db::core::init` — `__system__` is filtered out). Consequently
//! the public facade `ShamirDb::read_changelog_from("__system__", ...)`
//! returns `None`. The journal itself is sound — every system-store write
//! goes through `SystemStore::set_via_implicit_tx` →
//! `RepoInstance::run_implicit_batch_tx` → `commit_tx` →
//! `emit_changefeed_event`, so the events ARE produced on the underlying
//! `system` repo. This test reaches that repo directly via the
//! `pub(crate)` `SystemStore::system_repo()` accessor (the test module is
//! in-crate) and reads its durable journal.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{doc, insert};

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;

/// In-memory ShamirDb with a user db "testdb", repo "main", table "users"
/// (mirrors the layout used by the existing execute tests).
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;

    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));

    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Cross-check: a plain data-db write (`testdb/main/users`) is NOT
/// attributed to the system repo. This guards against a false positive
/// where the assertion above could pass because of a user-db write
/// leaking into the system journal rather than the admin op landing
/// there.
#[tokio::test]
async fn user_db_write_does_not_pollute_system_journal() {
    let shamir = setup().await;

    // A data write on the USER db — must land on `testdb/main`, NOT on the
    // system repo's journal.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        insert("users").row(doc().set("name", "Alice").set("age", 30)),
    );
    let req = b.to_request_via_msgpack();
    shamir.execute("testdb", &req).await.unwrap();

    let repo = shamir
        .system_store()
        .system_repo()
        .expect("system repo must exist");
    let jr = repo
        .read_changelog_from(1, 100)
        .await
        .expect("journal read");

    // The system journal may legitimately contain bootstrap events
    // (e.g. the `testdb` database catalogue row, the `main` repo row).
    // What it must NOT contain is an event for the USER-db `users`
    // insert above. We assert by attribution: every system-repo event's
    // `.repo == "system"`, never `"main"` (which is the user-db repo).
    assert!(
        jr.events.iter().all(|e| e.repo == "system"),
        "user-db writes must not appear on the system repo's journal; \
         found non-system attribution: {:?}",
        jr.events.iter().map(|e| &e.repo).collect::<Vec<_>>()
    );
    // And specifically no `users`-table event for the data write.
    assert!(
        !jr.events
            .iter()
            .flat_map(|e| e.changes.iter())
            .any(|c| c.table == "users"),
        "the user-db `users` insert must not emit a `users` event on the \
         system journal (it belongs to the user db, repo `main`); got tables: {:?}",
        jr.events
            .iter()
            .flat_map(|e| e.changes.iter())
            .map(|c| c.table.as_str())
            .collect::<Vec<_>>()
    );
}

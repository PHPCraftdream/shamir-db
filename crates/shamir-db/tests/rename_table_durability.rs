//! Phase F.2 — Durability test for populated-table RENAME.
//!
//! Proves that data committed BEFORE a rename survives a full
//! drop-and-reopen cycle (simulating a cold restart):
//!
//! 1. Create a table on a durable (fjall) system store.
//! 2. Insert N rows.
//! 3. Rename the table (force-drains the MVCC overlay into `__history__`,
//!    then copies stores).
//! 4. Drop the ShamirDb (cold restart).
//! 5. Reopen on the same path.
//! 6. Query the renamed table — ALL rows must be present.
//!
//! This is the critical durability gate: if `drain_to_history` failed to
//! land overlay entries in `__history__` before the store copy, the renamed
//! table would be empty after restart (silent data loss).

use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;

/// Re-open the system store, retrying briefly while the previous session's
/// store still holds the file lock (mirrors `durable_repo_tests`).
async fn reinit_with_retry(sys_path: std::path::PathBuf) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone())).await {
            Ok(shamir) => return shamir,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .expect("system store still locked after retries")
}

/// Populated table rename survives a cold restart: every committed row
/// written before the rename is readable from the renamed table after
/// drop-and-reopen.
#[tokio::test]
async fn rename_populated_survives_cold_restart() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // === Session 1: create db + repo + table, insert rows, rename ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("appdb").await;

        // Create repo (durable — no engine specified) + table.
        let mut b = Batch::new();
        b.id(1);
        b.create_repo("cr", ddl::create_repo("main").tables(["users"]));
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        // Insert 3 rows.
        for name in ["Alice", "Bob", "Carol"] {
            let mut b = Batch::new();
            b.id(2);
            b.insert("ins", insert("users").row(doc! { "name" => name }));
            shamir
                .execute("appdb", &b.to_request_via_msgpack())
                .await
                .unwrap();
        }

        // Rename users → people (force-drains overlay before copy).
        let mut b = Batch::new();
        b.id(3);
        b.rename_table("rn", ddl::rename_table("users", "people").repo("main"));
        let resp = shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        assert_eq!(
            resp.results["rn"].records[0].get_value_str("renamed_table"),
            Some("users")
        );

        // Verify data is readable BEFORE restart (sanity).
        let mut b = Batch::new();
        b.id(4);
        b.query("all", Query::from("people"));
        let resp = shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        assert_eq!(
            resp.results["all"].records.len(),
            3,
            "pre-restart: renamed table must have 3 rows"
        );
    } // ShamirDb dropped here — cold restart.

    // === Session 2: reopen on the SAME path, read back ===
    let shamir = reinit_with_retry(sys_path).await;

    // The renamed table must resolve.
    let mut b = Batch::new();
    b.id(1);
    b.query("all", Query::from("people"));
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let records = &resp.results["all"].records;
    assert_eq!(
        records.len(),
        3,
        "post-restart: renamed table must still have all 3 rows (durability)"
    );

    // Verify specific names survived.
    let names: shamir_collections::TFxSet<_> = records
        .iter()
        .filter_map(|r| r.get_value_str("name").map(|s| s.to_string()))
        .collect();
    assert!(names.contains("Alice"), "Alice must survive restart");
    assert!(names.contains("Bob"), "Bob must survive restart");
    assert!(names.contains("Carol"), "Carol must survive restart");

    // The old name must NOT resolve after restart.
    let mut b = Batch::new();
    b.id(2);
    b.query("old", Query::from("users"));
    let result = shamir.execute("appdb", &b.to_request_via_msgpack()).await;
    assert!(
        result.is_err(),
        "old table name must not resolve after restart"
    );
}

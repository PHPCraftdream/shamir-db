//! F-33 Step 4 (#838) — DDL-surface coverage for `CREATE REPO ... ENGINE
//! 'hybrid'`.
//!
//! Step 2 (#836) added `BoxRepo::Hybrid` / `BoxRepoFactory::Hybrid` in
//! `shamir-engine` (ephemeral in-memory table data, durable fjall mirror of
//! `__info__`/`__interner__`). Step 3 (#837) proved `TableManager::create`'s
//! open path tolerates this across a restart when the factory is built by
//! hand. This step wires `hybrid` into the real `CREATE REPO` DDL surface —
//! these tests prove the end-to-end path: DDL -> persisted system-store
//! record -> restart -> reattach, mirroring `durable_repo_tests.rs`'s
//! established wire-created-repo-restart shape (same `reinit_with_retry`
//! pattern, same `SystemStoreConfig::Fjall` durable home).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write;
use shamir_query_builder::Query;

use crate::shamir_db::SystemStoreConfig;
use crate::ShamirDb;

/// Re-open the system store, retrying briefly while the previous session's
/// store still holds the redb file lock. Mirrors `durable_repo_tests`'s
/// helper of the same name.
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

/// `CREATE REPO ... ENGINE 'hybrid'` against a durable `ShamirDb` (a
/// `data_root` exists) must succeed, and the table config
/// (index + schema/validator) created in it must survive a full `ShamirDb`
/// restart against the SAME `data_root` — while a row inserted into the
/// table must NOT survive, since hybrid keeps table data ephemeral
/// in-memory. This is the DDL-surface equivalent of Step 3's lower-level
/// `BoxRepoFactory::hybrid(...)`-built proof, now through the real
/// `CREATE REPO` / `ShamirDb::new` / reattach-on-boot path.
#[tokio::test]
async fn hybrid_repo_config_survives_restart_but_data_is_ephemeral() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // === Session 1: CREATE REPO ... ENGINE 'hybrid', add an index +
    // schema (validator), insert a row ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("appdb").await;

        let mut b = Batch::new();
        b.id(1);
        b.create_repo(
            "cr",
            ddl::create_repo("hyrepo")
                .engine("hybrid")
                .tables(["items"]),
        );
        let resp = shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        assert_eq!(
            resp.results["cr"].records[0].get_value_str("created_repo"),
            Some("hyrepo")
        );

        // Index on "name" (table config -> __info__ mirror, durable).
        let mut b = Batch::new();
        b.id(2);
        b.create_index(
            "idx",
            ddl::create_index("name_idx", "items")
                .repo("hyrepo")
                .field("name"),
        );
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        // Schema / validator (also table config -> __info__ mirror).
        let mut b = Batch::new();
        b.id(3);
        b.set_table_schema(
            "sch",
            ddl::set_table_schema("items")
                .repo("hyrepo")
                .rules([ddl::field(["name"]).string().required().build()]),
        );
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        // Insert a row (table DATA -> ephemeral in-memory, must NOT survive).
        let mut b = Batch::new();
        b.id(4);
        b.insert(
            "ins",
            write::Insert::with_repo("hyrepo", "items").row(doc! {
                "name" => "widget",
            }),
        );
        let resp = shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        assert_eq!(resp.results["ins"].records.len(), 1);
    }

    // === Session 2: reopen on the SAME meta path ===
    let shamir = reinit_with_retry(sys_path).await;
    let db = shamir.get_db("appdb").expect("db must survive restart");
    assert!(
        db.has_repo("hyrepo"),
        "hybrid repo must be re-attached after restart"
    );
    assert!(
        db.has_table("hyrepo", "items"),
        "table catalogue must be restored on init"
    );

    // Table config (index + schema/validator) must have survived — checked
    // via the real DescribeTable DDL introspection surface.
    let mut b = Batch::new();
    b.id(5);
    b.describe_table("desc", ddl::describe_table("items").repo("hyrepo"));
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let d = resp.results["desc"].records[0].as_value().as_ref().clone();

    let indexes = d
        .get("indexes")
        .and_then(|v| v.as_array())
        .expect("indexes section missing");
    assert!(
        indexes
            .iter()
            .any(|i| i.get("name").and_then(|v| v.as_str()) == Some("name_idx")),
        "name_idx must survive restart (hybrid mirrors __info__ durably)"
    );

    let validators = d
        .get("validators")
        .and_then(|v| v.as_array())
        .expect("validators section missing");
    assert!(
        !validators.is_empty(),
        "schema validator must survive restart (hybrid mirrors __info__ durably)"
    );

    // But the inserted row must be GONE — table data is ephemeral in-memory.
    let mut b = Batch::new();
    b.id(6);
    b.query("r", Query::with_repo("hyrepo", "items"));
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        resp.results["r"].records.len(),
        0,
        "hybrid repo table data must NOT survive restart (ephemeral by design)"
    );
}

/// `CREATE REPO ... ENGINE 'hybrid'` against a `ShamirDb` with NO
/// `data_root` (an in-memory-only home) must return a clear error — NOT a
/// silent `in_memory()` downgrade. Unlike the `fjall`/unspecified-engine
/// arm (which sensibly falls back to `in_memory()` when the caller didn't
/// insist on durability), an explicit `ENGINE 'hybrid'` request is an
/// explicit durability promise for the table config mirror, so silently
/// dropping it would be a correctness surprise.
#[tokio::test]
async fn hybrid_repo_without_data_root_errors_clearly() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("appdb").await;

    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("hyrepo").engine("hybrid"));
    let result = shamir.execute("appdb", &b.to_request_via_msgpack()).await;

    assert!(
        result.is_err(),
        "hybrid engine with no data_root must fail, not silently downgrade to in_memory"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("data_root") || err.contains("hybrid"),
        "error should clearly mention the missing data_root / hybrid engine, got: {err}"
    );

    // And the repo must NOT have been created at all.
    assert!(
        !shamir
            .get_db("appdb")
            .expect("db exists")
            .has_repo("hyrepo"),
        "repo must not exist after a failed hybrid create"
    );
}

/// `extract_storage_type` / `extract_path` round-trip: a hybrid repo's
/// persisted system-store record must read back `engine == "hybrid"` and
/// `path` set to the expected `<data_root>/<db>/<repo>` directory —
/// mirroring `durable_repo_file_mirrors_db_repo_tree`'s fjall check.
#[tokio::test]
async fn hybrid_repo_persists_engine_and_path_in_system_store() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta");

    let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
        .await
        .unwrap();
    shamir.create_db("appdb").await;

    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("hyrepo").engine("hybrid"));
    shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let repos = shamir.system_store().load_repositories().await.unwrap();
    let record = repos
        .iter()
        .find(|r| r["db_name"] == "appdb" && r["repo_name"] == "hyrepo")
        .expect("hybrid repo record must be persisted");

    assert_eq!(
        record["engine"].as_str(),
        Some("hybrid"),
        "persisted engine field must read back 'hybrid'"
    );

    let expected_path = dir.path().join("appdb").join("hyrepo");
    assert_eq!(
        record["path"].as_str(),
        Some(expected_path.to_string_lossy().as_ref()),
        "persisted path must be the hybrid repo's info_path directory"
    );

    // The directory must actually exist on disk (fjall __info__/__interner__
    // mirror), mirroring `durable_repo_file_mirrors_db_repo_tree`.
    assert!(
        expected_path.exists(),
        "hybrid repo's info directory must exist at {}",
        expected_path.display()
    );
}

/// F-33 Step 5 (#839) — a schema validator that survived a hybrid-repo
/// restart must genuinely still be RUNNING post-restart, not merely present
/// as inert metadata: a write that violates it must be rejected, and a write
/// that satisfies it must succeed.
#[tokio::test]
async fn hybrid_repo_validator_genuinely_enforces_post_restart() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // === Session 1: CREATE REPO ... ENGINE 'hybrid' + a required-field schema ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("appdb").await;

        let mut b = Batch::new();
        b.id(1);
        b.create_repo(
            "cr",
            ddl::create_repo("hyrepo")
                .engine("hybrid")
                .tables(["items"]),
        );
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        let mut b = Batch::new();
        b.id(2);
        b.set_table_schema(
            "sch",
            ddl::set_table_schema("items")
                .repo("hyrepo")
                .rules([ddl::field(["name"]).string().required().build()]),
        );
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
    }

    // === Session 2: reopen on the SAME meta path ===
    let shamir = reinit_with_retry(sys_path).await;

    // A write that VIOLATES the schema (missing the required "name" field)
    // must be rejected with a validation error, not merely have the rule
    // still listed in DescribeTable.
    let mut b = Batch::new();
    b.id(3);
    b.insert(
        "bad",
        write::Insert::with_repo("hyrepo", "items").row(doc! {
            "other" => "value",
        }),
    );
    let bad_result = shamir.execute("appdb", &b.to_request_via_msgpack()).await;
    assert!(
        bad_result.is_err(),
        "insert missing the required field must be rejected post-restart"
    );
    let err = bad_result.unwrap_err().to_string();
    assert!(
        err.contains("missing_required"),
        "error should mention missing_required, got: {err}"
    );

    // A write that SATISFIES the schema must succeed, proving the validator
    // is actively running (not just inert metadata).
    let mut b = Batch::new();
    b.id(4);
    b.insert(
        "good",
        write::Insert::with_repo("hyrepo", "items").row(doc! {
            "name" => "widget",
        }),
    );
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        resp.results["good"].records.len(),
        1,
        "insert satisfying the schema must succeed post-restart"
    );
}

/// F-33 Step 5 (#839) — a surviving index must be genuinely USABLE for a
/// brand-new post-restart write, which is also the strongest possible
/// functional proof of interner coherence: if the post-restart interner
/// resolved "name" to a different id than the one baked into the surviving
/// index definition, this query would silently return zero results instead
/// of erroring.
#[tokio::test]
async fn hybrid_repo_index_genuinely_usable_post_restart() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // === Session 1: CREATE REPO ... ENGINE 'hybrid', index on "name", insert
    // a row, confirm the index finds it pre-restart ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("appdb").await;

        let mut b = Batch::new();
        b.id(1);
        b.create_repo(
            "cr",
            ddl::create_repo("hyrepo")
                .engine("hybrid")
                .tables(["items"]),
        );
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        let mut b = Batch::new();
        b.id(2);
        b.create_index(
            "idx",
            ddl::create_index("name_idx", "items")
                .repo("hyrepo")
                .field("name"),
        );
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        let mut b = Batch::new();
        b.id(3);
        b.insert(
            "ins",
            write::Insert::with_repo("hyrepo", "items").row(doc! {
                "name" => "old-widget",
            }),
        );
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        // Pre-restart baseline: the index finds the row.
        let mut b = Batch::new();
        b.id(4);
        b.query(
            "r",
            Query::with_repo("hyrepo", "items").where_eq("name", "old-widget"),
        );
        let resp = shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        assert_eq!(
            resp.results["r"].records.len(),
            1,
            "pre-restart baseline: index must find the row it was built on"
        );
    }

    // === Session 2: reopen on the SAME meta path ===
    let shamir = reinit_with_retry(sys_path).await;

    // Insert a DIFFERENT row with a NEW value on the same indexed field.
    let mut b = Batch::new();
    b.id(5);
    b.insert(
        "ins2",
        write::Insert::with_repo("hyrepo", "items").row(doc! {
            "name" => "new-widget",
        }),
    );
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(resp.results["ins2"].records.len(), 1);

    // The query MUST find the new row via the surviving index. A silent
    // interner-id mismatch between the post-restart repo and the surviving
    // index definition would make this return 0 hits instead of erroring.
    let mut b = Batch::new();
    b.id(6);
    b.query(
        "r_new",
        Query::with_repo("hyrepo", "items").where_eq("name", "new-widget"),
    );
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        resp.results["r_new"].records.len(),
        1,
        "surviving index must be usable for a brand-new post-restart write \
         (interner must resolve 'name' to the same id the index was built on)"
    );

    // No stale postings, DDL-level: querying for the OLD pre-restart value
    // via the SAME index must return 0 hits (the row itself is gone —
    // table data is ephemeral by design).
    let mut b = Batch::new();
    b.id(7);
    b.query(
        "r_old",
        Query::with_repo("hyrepo", "items").where_eq("name", "old-widget"),
    );
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        resp.results["r_old"].records.len(),
        0,
        "querying the pre-restart value via the surviving index must find \
         nothing post-restart (no stale postings, data is ephemeral)"
    );
}

/// F-33 Step 5 (#839) — two full restart cycles in a row: the surviving
/// index must stay usable and coherent on the SECOND reattach too, not just
/// the first. A "warm-then-cold-again" open sequence is a plausible spot for
/// drift (e.g. accidentally re-persisting/duplicating index state, or losing
/// it) that a single-restart test would not catch.
#[tokio::test]
async fn hybrid_repo_index_stays_usable_across_two_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // === Session 1: CREATE REPO ... ENGINE 'hybrid', index on "name" ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("appdb").await;

        let mut b = Batch::new();
        b.id(1);
        b.create_repo(
            "cr",
            ddl::create_repo("hyrepo")
                .engine("hybrid")
                .tables(["items"]),
        );
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        let mut b = Batch::new();
        b.id(2);
        b.create_index(
            "idx",
            ddl::create_index("name_idx", "items")
                .repo("hyrepo")
                .field("name"),
        );
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        let mut b = Batch::new();
        b.id(3);
        b.insert(
            "ins",
            write::Insert::with_repo("hyrepo", "items").row(doc! {
                "name" => "gen1",
            }),
        );
        shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
    }

    // === Session 2: reopen after FIRST restart, write a new generation-2
    // row, confirm both the new write and the surviving config are usable ===
    {
        let shamir = reinit_with_retry(sys_path.clone()).await;

        let mut b = Batch::new();
        b.id(4);
        b.insert(
            "ins2",
            write::Insert::with_repo("hyrepo", "items").row(doc! {
                "name" => "gen2",
            }),
        );
        let resp = shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        assert_eq!(resp.results["ins2"].records.len(), 1);

        let mut b = Batch::new();
        b.id(5);
        b.query(
            "r_gen2",
            Query::with_repo("hyrepo", "items").where_eq("name", "gen2"),
        );
        let resp = shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        assert_eq!(
            resp.results["r_gen2"].records.len(),
            1,
            "index must be usable for a new write after the FIRST restart"
        );

        let mut b = Batch::new();
        b.id(6);
        b.query(
            "r_gen1",
            Query::with_repo("hyrepo", "items").where_eq("name", "gen1"),
        );
        let resp = shamir
            .execute("appdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        assert_eq!(
            resp.results["r_gen1"].records.len(),
            0,
            "generation-1 row must be gone after the FIRST restart (ephemeral data)"
        );
    }

    // === Session 3: reopen after the SECOND restart in a row ===
    let shamir = reinit_with_retry(sys_path).await;

    // DescribeTable: the index definition must still be there — no drift on
    // the second reattach (e.g. duplication, or silent loss).
    let mut b = Batch::new();
    b.id(7);
    b.describe_table("desc", ddl::describe_table("items").repo("hyrepo"));
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let d = resp.results["desc"].records[0].as_value().as_ref().clone();
    let indexes = d
        .get("indexes")
        .and_then(|v| v.as_array())
        .expect("indexes section missing");
    assert_eq!(
        indexes
            .iter()
            .filter(|i| i.get("name").and_then(|v| v.as_str()) == Some("name_idx"))
            .count(),
        1,
        "name_idx must survive the SECOND restart exactly once (no duplication/loss)"
    );

    // A brand-new generation-3 write must be found via the same index —
    // confirming interner coherence holds on the SECOND reattach too.
    let mut b = Batch::new();
    b.id(8);
    b.insert(
        "ins3",
        write::Insert::with_repo("hyrepo", "items").row(doc! {
            "name" => "gen3",
        }),
    );
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(resp.results["ins3"].records.len(), 1);

    let mut b = Batch::new();
    b.id(9);
    b.query(
        "r_gen3",
        Query::with_repo("hyrepo", "items").where_eq("name", "gen3"),
    );
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        resp.results["r_gen3"].records.len(),
        1,
        "index must remain usable for a new write after the SECOND restart"
    );

    // And BOTH prior generations' data must remain gone (ephemeral, no
    // resurrection/duplication across the second reattach either).
    let mut b = Batch::new();
    b.id(10);
    b.query(
        "r_all_old",
        Query::with_repo("hyrepo", "items").where_eq("name", "gen2"),
    );
    let resp = shamir
        .execute("appdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        resp.results["r_all_old"].records.len(),
        0,
        "generation-2 row must still be gone after the SECOND restart"
    );
}

/// The unsupported-engine error message must mention `hybrid` alongside
/// `in_memory`/`fjall` now that it's a real supported choice.
#[tokio::test]
async fn unsupported_engine_error_mentions_hybrid() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("appdb").await;

    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("badrepo").engine("bogus_engine"));
    let result = shamir.execute("appdb", &b.to_request_via_msgpack()).await;

    assert!(result.is_err(), "unknown engine must be rejected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("hybrid"),
        "unsupported-engine error should mention 'hybrid', got: {err}"
    );
    assert!(
        err.contains("in_memory") && err.contains("fjall"),
        "unsupported-engine error should still mention in_memory/fjall, got: {err}"
    );
}

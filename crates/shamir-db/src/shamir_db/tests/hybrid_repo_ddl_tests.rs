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

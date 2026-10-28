//! Round-trip tests for the `interner.dump` / `interner.touch` wire ops
//! (Stage 5d). Ops are built via the query-builder and executed through
//! the real pipeline (msgpack → BatchOp → admin executor).

use shamir_collections::TFxMap;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_storage::types::Repo;
use shamir_types::core::interner::InternerQuota;
use shamir_types::types::value::QueryValue;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await.expect("create_db");
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Like [`setup_shamir`], but registers the repo through `ShamirDb::add_repo`
/// so its Store catalogue row (and therefore its ACL meta) exists.
async fn setup_catalogued_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await.expect("create_db");
    shamir
        .add_repo(
            "testdb",
            RepoConfig::new("main", BoxRepoFactory::in_memory())
                .add_table(TableConfig::new("users")),
        )
        .await
        .unwrap();
    shamir
}

async fn setup_shamir_with_quota(quota: InternerQuota) -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await.expect("create_db");
    let instance = shamir_engine::repo::RepoInstance::from_factory_with_interner_quota_for_test(
        "main".to_string(),
        BoxRepoFactory::in_memory(),
        Vec::new(),
        quota,
    )
    .await
    .unwrap();
    let permit = instance.write_domain().admit().await.unwrap();
    db.publish_repo_admitted(instance, &permit).unwrap();
    drop(permit);
    shamir
}

/// Run a single admin op through the real pipeline, returning the first
/// result record (the QueryValue the handler wrapped via admin_result).
async fn run_one(
    shamir: &ShamirDb,
    op: impl shamir_query_builder::batch::IntoBatchOp,
) -> QueryValue {
    let mut b = Batch::new();
    b.id(1);
    b.op("r", op);
    let req = b.to_request_via_msgpack().expect("batch request encoding");
    let resp = shamir.execute("testdb", &req).await.unwrap();
    resp.results["r"].records[0].as_value().into_owned()
}

#[tokio::test]
async fn rv_db_4d93f623_ambient_delta_requires_store_read() {
    let shamir = setup_catalogued_shamir().await;
    shamir
        .set_resource_meta(
            &crate::access::ResourcePath::database("testdb"),
            &crate::access::ResourceMeta::open(),
        )
        .await
        .unwrap();
    let denied_meta = crate::access::ResourceMeta {
        owner: crate::access::Actor::User(1),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(
            &crate::access::ResourcePath::store("testdb", "main"),
            &denied_meta,
        )
        .await
        .unwrap();
    let mut request = shamir_query_builder::batch::Batch::new();
    request.id("ambient-denied");
    let mut request = request
        .to_request_via_msgpack()
        .expect("batch request encoding");
    request.interner_epochs.insert("main".into(), 0);

    let response = shamir
        .execute_as(crate::access::Actor::User(99), "testdb", &request)
        .await
        .expect("denied ambient repositories are skipped");
    assert!(!response.interner_delta.contains_key("main"));
}

#[tokio::test]
async fn rv_db_4d93f623_ambient_delta_succeeds_with_store_read() {
    let shamir = setup_catalogued_shamir().await;
    shamir
        .set_resource_meta(
            &crate::access::ResourcePath::database("testdb"),
            &crate::access::ResourceMeta::open(),
        )
        .await
        .unwrap();
    shamir
        .set_resource_meta(
            &crate::access::ResourcePath::store("testdb", "main"),
            &crate::access::ResourceMeta::open(),
        )
        .await
        .unwrap();
    let mut batch = shamir_query_builder::batch::Batch::new();
    batch.id("ambient-allowed");
    let mut request = batch
        .to_request_via_msgpack()
        .expect("batch request encoding");
    request.interner_epochs.insert("main".into(), 0);
    let response = shamir
        .execute_as(crate::access::Actor::User(99), "testdb", &request)
        .await
        .unwrap();
    assert!(response.interner_delta.contains_key("main"));
}

#[tokio::test]
async fn rv_types_2eb39dfd_touch_quota_persists_prefix_without_rejected_id() {
    let shamir = setup_shamir_with_quota(InternerQuota {
        max_names: 1,
        max_bytes: 1024,
    })
    .await;
    let repo = shamir
        .get_db("testdb")
        .and_then(|db| db.get_repo("main"))
        .expect("repo exists");
    let backing = repo.seed_backing_repo();
    let interner_store = backing
        .store_get("__interner__")
        .await
        .expect("interner store");

    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "touch",
        ddl::interner_touch(["accepted_prefix", "rejected_suffix"]),
    );
    let request = batch
        .to_request_via_msgpack()
        .expect("batch request encoding");
    let error = shamir
        .execute("testdb", &request)
        .await
        .expect_err("touch must reject the over-quota suffix");
    assert!(
        error.to_string().contains("interner_quota_exceeded"),
        "{error:?}"
    );

    // Hydrating through a second manager exercises the persisted dictionary.
    let reopened = shamir_engine::table::InternerManager::new(interner_store);
    let interner = reopened.get().await.expect("reopen dictionary");
    assert!(interner.get_ind("accepted_prefix").is_some());
    assert!(interner.get_ind("rejected_suffix").is_none());
    assert_eq!(interner.generation(), 1, "rejected suffix consumes no id");
}

#[tokio::test]
async fn touch_assigns_distinct_ids_and_42_is_not_id_42() {
    let shamir = setup_shamir().await;

    let out = run_one(&shamir, ddl::interner_touch(["age", "name", "42"])).await;

    // mappings is [["age",id],["name",id],["42",id]]
    let mappings = out.get("mappings").and_then(|v| v.as_array()).unwrap();
    assert_eq!(mappings.len(), 3, "expected 3 mappings, got {mappings:?}");

    let ids: Vec<u64> = mappings
        .iter()
        .map(|m| m.as_array().unwrap().get(1).unwrap().as_u64().unwrap())
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        3,
        "ids must be 3 distinct values, got {ids:?}"
    );

    // §9.4: "42" is the STRING "42", not raw id 42.
    let id_of_42 = mappings
        .iter()
        .find(|m| {
            m.as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                == Some("42")
        })
        .map(|m| m.as_array().unwrap().get(1).unwrap().as_u64().unwrap())
        .expect("must have a mapping for \"42\"");
    assert_ne!(
        id_of_42, 42,
        "§9.4: \"42\" must intern to the interner-assigned id, not raw id 42"
    );
}

#[tokio::test]
async fn dump_returns_touched_pairs_and_epoch() {
    let shamir = setup_shamir().await;

    // Seed three names.
    let touch_out = run_one(&shamir, ddl::interner_touch(["age", "name", "42"])).await;
    let touch_map: TFxMap<String, u64> = touch_out
        .get("mappings")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .map(|m| {
            let pair = m.as_array().unwrap();
            (
                pair.first().unwrap().as_str().unwrap().to_string(),
                pair.get(1).unwrap().as_u64().unwrap(),
            )
        })
        .collect();

    let out = run_one(&shamir, ddl::interner_dump()).await;
    assert_eq!(
        out.get("interner_dump").and_then(|v| v.as_str()).unwrap(),
        "main"
    );

    // entries is [[id,"name"],...]
    let entries = out.get("entries").and_then(|v| v.as_array()).unwrap();
    let by_id: TFxMap<u64, String> = entries
        .iter()
        .map(|e| {
            let pair = e.as_array().unwrap();
            (
                pair.first().unwrap().as_u64().unwrap(),
                pair.get(1).unwrap().as_str().unwrap().to_string(),
            )
        })
        .collect();
    for (name, id) in &touch_map {
        assert_eq!(
            by_id.get(id).map(String::as_str),
            Some(name.as_str()),
            "dump must contain id {id} → {name}"
        );
    }
    assert_eq!(entries.len(), 3, "dump must list all 3 entries");

    // epoch == the max id present (3 here — ids are 1-based and dense).
    let epoch = out.get("epoch").and_then(|v| v.as_u64()).unwrap();
    assert_eq!(epoch, 3, "epoch must be the max id present, got {epoch}");
}

#[tokio::test]
async fn retouch_is_idempotent() {
    let shamir = setup_shamir().await;

    let first = run_one(&shamir, ddl::interner_touch(["age", "name", "42"])).await;
    let id_age_first = first
        .get("mappings")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .find(|m| {
            m.as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                == Some("age")
        })
        .map(|m| m.as_array().unwrap().get(1).unwrap().as_u64().unwrap())
        .unwrap();

    // Re-touch only "age" — must return the SAME id.
    let second = run_one(&shamir, ddl::interner_touch(["age"])).await;
    let id_age_second = second
        .get("mappings")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .find(|m| {
            m.as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                == Some("age")
        })
        .map(|m| m.as_array().unwrap().get(1).unwrap().as_u64().unwrap())
        .unwrap();

    assert_eq!(
        id_age_first, id_age_second,
        "idempotent re-touch must return the same id"
    );
    // No new id was minted.
    assert_eq!(
        second
            .get("mappings")
            .and_then(|v| v.as_array())
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn dump_since_returns_only_delta() {
    let shamir = setup_shamir().await;

    // Seed 3 names → epoch 3.
    let seed = run_one(&shamir, ddl::interner_touch(["age", "name", "42"])).await;
    let prev_epoch = seed.get("epoch").and_then(|v| v.as_u64()).unwrap();
    assert_eq!(prev_epoch, 3);

    // Add one more → epoch 4.
    let more = run_one(&shamir, ddl::interner_touch(["city"])).await;
    assert_eq!(more.get("epoch").and_then(|v| v.as_u64()).unwrap(), 4);

    // Delta dump since the previous epoch must list ONLY "city".
    let out = run_one(&shamir, ddl::interner_dump().since(prev_epoch)).await;
    let entries = out.get("entries").and_then(|v| v.as_array()).unwrap();
    assert_eq!(entries.len(), 1, "delta dump must list only the new entry");
    let entry_pair = entries[0].as_array().unwrap();
    assert_eq!(entry_pair.get(1).unwrap().as_str().unwrap(), "city");
    assert_eq!(
        entry_pair.first().unwrap().as_u64().unwrap(),
        4,
        "delta entry must be id 4"
    );
    // The high-water epoch advanced to 4.
    assert_eq!(out.get("epoch").and_then(|v| v.as_u64()).unwrap(), 4);
}

#[tokio::test]
async fn dump_on_empty_repo_returns_empty_entries_and_epoch_zero() {
    let shamir = setup_shamir().await;

    let out = run_one(&shamir, ddl::interner_dump()).await;
    assert_eq!(
        out.get("entries").and_then(|v| v.as_array()).unwrap().len(),
        0,
        "fresh repo dump must have no entries"
    );
    assert_eq!(
        out.get("epoch").and_then(|v| v.as_u64()).unwrap(),
        0,
        "fresh repo epoch is 0"
    );
}

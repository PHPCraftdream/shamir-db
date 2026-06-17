//! Round-trip tests for the `interner.dump` / `interner.touch` wire ops
//! (Stage 5d). Ops are built via the query-builder and executed through
//! the real pipeline (msgpack → BatchOp → admin executor).

use serde_json::Value;
use shamir_collections::TFxMap;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;

    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));

    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Run a single admin op through the real pipeline, returning the first
/// result record (the JSON object the handler wrapped via admin_result).
async fn run_one(shamir: &ShamirDb, op: impl shamir_query_builder::batch::IntoBatchOp) -> Value {
    let mut b = Batch::new();
    b.id(1);
    b.op("r", op);
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    resp.results["r"].records[0].as_json().into_owned()
}

#[tokio::test]
async fn touch_assigns_distinct_ids_and_42_is_not_id_42() {
    let shamir = setup_shamir().await;

    let out = run_one(&shamir, ddl::interner_touch(["age", "name", "42"])).await;

    // mappings is [["age",id],["name",id],["42",id]]
    let mappings = out["mappings"].as_array().unwrap();
    assert_eq!(mappings.len(), 3, "expected 3 mappings, got {mappings:?}");

    let ids: Vec<u64> = mappings.iter().map(|m| m[1].as_u64().unwrap()).collect();
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
        .find(|m| m[0].as_str() == Some("42"))
        .map(|m| m[1].as_u64().unwrap())
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
    let touch_map: TFxMap<String, u64> = touch_out["mappings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m[0].as_str().unwrap().to_string(), m[1].as_u64().unwrap()))
        .collect();

    let out = run_one(&shamir, ddl::interner_dump()).await;
    assert_eq!(out["interner_dump"].as_str().unwrap(), "main");

    // entries is [[id,"name"],...]
    let entries = out["entries"].as_array().unwrap();
    let by_id: TFxMap<u64, String> = entries
        .iter()
        .map(|e| (e[0].as_u64().unwrap(), e[1].as_str().unwrap().to_string()))
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
    let epoch = out["epoch"].as_u64().unwrap();
    assert_eq!(epoch, 3, "epoch must be the max id present, got {epoch}");
}

#[tokio::test]
async fn retouch_is_idempotent() {
    let shamir = setup_shamir().await;

    let first = run_one(&shamir, ddl::interner_touch(["age", "name", "42"])).await;
    let id_age_first = first["mappings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m[0].as_str() == Some("age"))
        .map(|m| m[1].as_u64().unwrap())
        .unwrap();

    // Re-touch only "age" — must return the SAME id.
    let second = run_one(&shamir, ddl::interner_touch(["age"])).await;
    let id_age_second = second["mappings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m[0].as_str() == Some("age"))
        .map(|m| m[1].as_u64().unwrap())
        .unwrap();

    assert_eq!(
        id_age_first, id_age_second,
        "idempotent re-touch must return the same id"
    );
    // No new id was minted.
    assert_eq!(second["mappings"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn dump_since_returns_only_delta() {
    let shamir = setup_shamir().await;

    // Seed 3 names → epoch 3.
    let seed = run_one(&shamir, ddl::interner_touch(["age", "name", "42"])).await;
    let prev_epoch = seed["epoch"].as_u64().unwrap();
    assert_eq!(prev_epoch, 3);

    // Add one more → epoch 4.
    let more = run_one(&shamir, ddl::interner_touch(["city"])).await;
    assert_eq!(more["epoch"].as_u64().unwrap(), 4);

    // Delta dump since the previous epoch must list ONLY "city".
    let out = run_one(&shamir, ddl::interner_dump().since(prev_epoch)).await;
    let entries = out["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "delta dump must list only the new entry");
    assert_eq!(entries[0][1].as_str().unwrap(), "city");
    assert_eq!(
        entries[0][0].as_u64().unwrap(),
        4,
        "delta entry must be id 4"
    );
    // The high-water epoch advanced to 4.
    assert_eq!(out["epoch"].as_u64().unwrap(), 4);
}

#[tokio::test]
async fn dump_on_empty_repo_returns_empty_entries_and_epoch_zero() {
    let shamir = setup_shamir().await;

    let out = run_one(&shamir, ddl::interner_dump()).await;
    assert_eq!(
        out["entries"].as_array().unwrap().len(),
        0,
        "fresh repo dump must have no entries"
    );
    assert_eq!(out["epoch"].as_u64().unwrap(), 0, "fresh repo epoch is 0");
}

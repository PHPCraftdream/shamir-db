//! S5c-X PR8b-2 evidence row 4 — legacy / malformed / unsupported-version
//! subscription rows are refused at OPEN with the typed
//! `DbError::SubscriptionCatalogue` (wire code `subscription_catalogue_invalid`)
//! naming the subscription, for active AND paused rows alike; a fully valid
//! new-format row reopens fine (negative control).
//!
//! Reopen leg note: this test only exercises the persisted system store
//! (`SystemStoreConfig::Fjall`) — no seed installer is involved — so unlike
//! the recovery tests it does NOT need `#[cfg(unix)]` and runs on every
//! platform.

use shamir_types::mpack;

use crate::query::write::SetOp;
use crate::query::TableRef;
use crate::shamir_db::SystemStoreConfig;
use crate::types::common::new_map;
use crate::types::value::QueryValue;
use crate::{DbError, ShamirDb};

/// Build a minimal subscription row with the legacy (binding-less) shape.
fn legacy_row(name: &str, state: &str) -> QueryValue {
    let mut m = new_map();
    m.insert("name".to_string(), QueryValue::Str(name.to_string()));
    m.insert(
        "upstream".to_string(),
        QueryValue::Str("tcp://peer:9000".to_string()),
    );
    m.insert(
        "publication".to_string(),
        QueryValue::Str("pub".to_string()),
    );
    m.insert("profile".to_string(), QueryValue::Str("prof".to_string()));
    m.insert("state".to_string(), QueryValue::Str(state.to_string()));
    QueryValue::Map(m)
}

/// Overwrite the `binding_destinations` field with `value` (malformed shapes).
fn with_destinations(row: QueryValue, value: QueryValue) -> QueryValue {
    if let QueryValue::Map(ref mut m) = row {
        m.insert("binding_destinations".to_string(), value);
    }
    row
}

/// Overwrite the `binding_version` field (unsupported shapes).
fn with_version(row: QueryValue, version: u64) -> QueryValue {
    if let QueryValue::Map(ref mut m) = row {
        m.insert(
            "binding_version".to_string(),
            QueryValue::Int(version as i64),
        );
    }
    row
}

/// Add a complete VALID binding over `(db, repo)` to the row.
fn with_valid_binding(row: QueryValue, db: &str, repo: &str) -> QueryValue {
    let mut entry = new_map();
    entry.insert("db".to_string(), QueryValue::Str(db.to_string()));
    entry.insert("repo".to_string(), QueryValue::Str(repo.to_string()));
    let mut row = with_destinations(row, QueryValue::List(vec![QueryValue::Map(entry)]));
    if let QueryValue::Map(ref mut m) = row {
        m.insert("binding_version".to_string(), QueryValue::Int(1));
        m.insert("binding_generation".to_string(), QueryValue::Int(1));
    }
    row
}

/// Persist `row` into `system/subscriptions` of a FRESH fjall system store,
/// close the instance, then attempt to reopen. Retries only on non-catalogue
/// errors (fjall releases its lock a few ms after the drop — mirrors
/// `durable_repo_tests`); the typed catalogue refusal short-circuits.
async fn open_after_persisting(
    row: QueryValue,
    sys_path: std::path::PathBuf,
) -> Result<ShamirDb, DbError> {
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        let table = shamir.system_store().subscriptions_table().await.unwrap();
        let name = row
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>")
            .to_string();
        let set_op = SetOp {
            set: TableRef::new("subscriptions"),
            key: mpack!({"name": @(QueryValue::Str(name))}),
            value: row,
        };
        shamir
            .system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await
            .unwrap();
        shamir
            .system_store()
            .persist_table_interner(&table, None)
            .await
            .unwrap();
        shamir.flush_seed_catalogue().await.unwrap();
    }

    let mut last = None;
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone())).await {
            Ok(opened) => return Ok(opened),
            Err(err) if err.code() == Some("subscription_catalogue_invalid") => return Err(err),
            Err(err) => {
                last = Some(err);
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
    }
    Err(last.expect("init must fail at least once"))
}

/// Assert the typed open refusal names `subscription`.
fn assert_catalogue_refusal(result: Result<ShamirDb, DbError>, subscription: &str) {
    let err = result.unwrap_err();
    assert_eq!(
        err.code(),
        Some("subscription_catalogue_invalid"),
        "open must fail with the typed catalogue code: {err}"
    );
    match err {
        DbError::SubscriptionCatalogue(payload) => {
            assert_eq!(payload.subscription, subscription, "{payload:?}");
            assert!(!payload.reason.is_empty());
        }
        other => panic!("expected DbError::SubscriptionCatalogue, got {other}"),
    }
}

/// Legacy row (no binding fields at all), `state = "active"`: typed open
/// refusal naming the subscription.
#[tokio::test]
async fn binding_open_refusal_legacy_active_row() {
    let dir = tempfile::tempdir().unwrap();
    let result = open_after_persisting(
        legacy_row("legacy_active", "active"),
        dir.path().join("meta"),
    )
    .await;
    assert_catalogue_refusal(result, "legacy_active");
}

/// Legacy row (no binding fields), `state = "paused"`: paused rows are
/// validated too.
#[tokio::test]
async fn binding_open_refusal_legacy_paused_row() {
    let dir = tempfile::tempdir().unwrap();
    let result = open_after_persisting(
        legacy_row("legacy_paused", "paused"),
        dir.path().join("meta"),
    )
    .await;
    assert_catalogue_refusal(result, "legacy_paused");
}

/// Malformed binding: a destination entry missing its `repo` string is a
/// typed open refusal.
#[tokio::test]
async fn binding_open_refusal_malformed_binding_destinations() {
    let dir = tempfile::tempdir().unwrap();
    let mut entry = new_map();
    entry.insert("db".to_string(), QueryValue::Str("testdb".to_string()));
    let row = with_version(
        with_destinations(
            legacy_row("malformed", "active"),
            QueryValue::List(vec![QueryValue::Map(entry)]),
        ),
        1,
    );
    // `binding_generation` present so the destinations check is what fires.
    let mut row = row;
    if let QueryValue::Map(ref mut m) = row {
        m.insert("binding_generation".to_string(), QueryValue::Int(1));
    }
    let result = open_after_persisting(row, dir.path().join("meta")).await;
    assert_catalogue_refusal(result, "malformed");
}

/// Unsupported binding version: a row recorded under a format this build does
/// not know is a typed open refusal.
#[tokio::test]
async fn binding_open_refusal_unsupported_binding_version() {
    let dir = tempfile::tempdir().unwrap();
    let mut entry = new_map();
    entry.insert("db".to_string(), QueryValue::Str("testdb".to_string()));
    entry.insert("repo".to_string(), QueryValue::Str("main".to_string()));
    let row = with_version(
        with_destinations(
            legacy_row("future_version", "active"),
            QueryValue::List(vec![QueryValue::Map(entry)]),
        ),
        99,
    );
    let mut row = row;
    if let QueryValue::Map(ref mut m) = row {
        m.insert("binding_generation".to_string(), QueryValue::Int(1));
    }
    let result = open_after_persisting(row, dir.path().join("meta")).await;
    assert_catalogue_refusal(result, "future_version");
}

/// Negative control: a fully valid new-format row reopens fine, and the
/// reopened instance serves ordinary reads.
#[tokio::test]
async fn binding_open_refusal_valid_row_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let row = with_valid_binding(legacy_row("valid_row", "active"), "testdb", "main");
    let shamir = open_after_persisting(row, dir.path().join("meta"))
        .await
        .expect("a fully valid row must reopen");
    // The catalogue validated, and an ACTIVE (non-quarantined) valid row
    // covers nothing.
    assert!(!shamir
        .has_resync_required_subscription_covering("testdb", "main")
        .await
        .unwrap());
}

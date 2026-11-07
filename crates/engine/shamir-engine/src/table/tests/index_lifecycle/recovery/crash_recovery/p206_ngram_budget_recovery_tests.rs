//! Ngram budget failures during Building recovery stay planner-invisible.

use std::sync::Arc;

use bytes::Bytes;
use shamir_query_builder::{ddl, filter as qf, Query};
use shamir_query_types::batch::BatchOp;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use crate::index2::backend::{FtsMode, IndexQuery, IndexResult};
use crate::index2::state::IndexState;
use crate::query::filter::FilterContext;
use crate::table::TableManager;

#[tokio::test]
async fn p206_ngram_budget_recovery_and_repair() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let table = TableManager::create("docs".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let interner = table.interner().get().await.unwrap();
    let key = interner.touch_ind("body").unwrap().into_key();
    table.interner().persist().await.unwrap();
    let mut row = new_map();
    row.insert(
        key.clone(),
        InnerValue::Str(format!("{} needle", "a".repeat(1_048_580))),
    );
    let rid = table.insert(&InnerValue::Map(row)).await.unwrap();
    let BatchOp::CreateIndex(op) = ddl::create_index("search", "docs")
        .field("body")
        .index_type("fts")
        .fts_tokenizer("ngram3")
        .build()
    else {
        panic!("expected CREATE INDEX")
    };
    let error = table.create_index_v2(&op).await.unwrap_err();
    assert!(error.to_string().contains("budget exceeded"));
    drop(table);

    let table = TableManager::create("docs".into(), data, info)
        .await
        .unwrap();
    let backends = table.index2_registry().all_backends().await;
    assert_eq!(backends.len(), 1);
    let id = backends[0].descriptor().id;
    assert_eq!(
        table.index2_registry().state_of(id).await,
        Some(IndexState::Failed)
    );
    let reason = table.index2_registry().failure_reason_of(id).await.unwrap();
    for expected in [
        "ngram token budget",
        "shorten or delete",
        "TableManager::repair()",
        "DROP INDEX",
        "Repair alone is insufficient",
    ] {
        assert!(reason.contains(expected), "missing {expected}: {reason}");
    }
    assert!(table
        .index2_registry()
        .lease_by_field_and_kind(&[key.id()], "fts")
        .await
        .unwrap()
        .is_none());

    // The scan fallback must not use the incomplete backend as an empty answer.
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let result = table
        .read(
            &Query::from("docs")
                .build()
                .filter(qf::fts("body", "needle", "and")),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
    drop(ctx);
    drop(backends);

    let mut corrected = new_map();
    corrected.insert(key.clone(), InnerValue::Str("needle".into()));
    table.set(rid, &InnerValue::Map(corrected)).await.unwrap();
    table.repair().await.unwrap();
    assert_eq!(
        table.index2_registry().state_of(id).await,
        Some(IndexState::Ready)
    );
    let lease = table
        .index2_registry()
        .lease_by_field_and_kind(&[key.id()], "fts")
        .await
        .unwrap()
        .unwrap();
    let tokens = lease.backend.tokenize_query("needle").unwrap();
    let result = lease
        .backend
        .lookup(IndexQuery::Fts {
            tokens,
            mode: FtsMode::AndAll,
            limit: None,
        })
        .await
        .unwrap();
    match result {
        IndexResult::Ranked(rows) => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].0, rid);
        }
        _ => panic!("expected ranked FTS results"),
    }
}

#[tokio::test]
async fn p206_nonbudget_backfill_error_propagates() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let table = TableManager::create("docs".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let interner = table.interner().get().await.unwrap();
    let key = interner.touch_ind("body").unwrap().into_key();
    table.interner().persist().await.unwrap();
    let mut row = new_map();
    row.insert(key, InnerValue::Str("a".repeat(1_048_580)));
    let rid = table.insert(&InnerValue::Map(row)).await.unwrap();
    let BatchOp::CreateIndex(op) = ddl::create_index("search", "docs")
        .field("body")
        .index_type("fts")
        .fts_tokenizer("ngram3")
        .build()
    else {
        panic!("expected CREATE INDEX")
    };
    assert!(table.create_index_v2(&op).await.is_err());
    drop(table);
    data.set(
        RecordKey::from_slice(rid.as_bytes()),
        Bytes::from_static(b"\xc4"),
    )
    .await
    .unwrap();
    assert!(TableManager::create("docs".into(), data, info)
        .await
        .is_err());
}

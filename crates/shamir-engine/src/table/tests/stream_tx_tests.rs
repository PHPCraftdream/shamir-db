//! Tests for tx-aware streaming wrappers on TableManager (Stage 3.2.B).
//!
//! At this stage the `*_tx` methods forward to their non-tx counterparts
//! regardless of `tx`. These tests pin that contract so future wiring
//! (3.2.B.2 / 3.3) can be sanity-checked without surprises.

use std::sync::Arc;

use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::table::record_cow::RecordCow;
use crate::table::TableManager;

async fn make_table_with_n_records(n: usize) -> (TableManager, Vec<RecordId>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let id = tbl.insert(&InnerValue::Str(format!("v{i}"))).await.unwrap();
        ids.push(id);
    }
    (tbl, ids)
}

fn make_tx(snapshot: u64) -> TxContext {
    TxContext::new(TxId::new(1), 0, snapshot, IsolationLevel::Snapshot)
}

async fn collect_stream<S>(stream: S) -> Vec<(RecordId, InnerValue)>
where
    S: futures::Stream<Item = shamir_storage::error::DbResult<Vec<(RecordId, RecordCow)>>>,
{
    futures::pin_mut!(stream);
    let mut out = Vec::new();
    while let Some(batch) = stream.next().await {
        for (id, cow) in batch.unwrap() {
            out.push((id, cow.into_inner().unwrap()));
        }
    }
    out
}

#[tokio::test]
async fn list_stream_tx_none_matches_list_stream() {
    let (tbl, _ids) = make_table_with_n_records(5).await;

    let baseline = collect_stream(tbl.list_stream(2)).await;
    let via_tx_none = collect_stream(tbl.list_stream_tx(None, 2)).await;
    assert_eq!(baseline.len(), via_tx_none.len());
    assert_eq!(baseline.len(), 5);
}

#[tokio::test]
async fn list_stream_tx_some_matches_list_stream_forward() {
    let (tbl, _ids) = make_table_with_n_records(3).await;
    let tx = make_tx(123);

    let baseline = collect_stream(tbl.list_stream(2)).await;
    let via_tx_some = collect_stream(tbl.list_stream_tx(Some(&tx), 2)).await;
    assert_eq!(baseline.len(), via_tx_some.len());
    assert_eq!(baseline.len(), 3);
}

#[tokio::test]
async fn filter_stream_with_callback_tx_forwards() {
    use crate::query::filter::eval::{compile_filter, FilterCallback};
    use crate::query::filter::eval_context::FilterContext;
    use crate::query::filter::Filter;
    use shamir_types::types::common::new_map;

    let (tbl, _ids) = make_table_with_n_records(4).await;
    let interner = tbl.interner().get().await.unwrap();

    let filter = Filter::And { filters: vec![] };
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let cb = compile_filter(&filter, interner);

    let baseline =
        collect_stream(tbl.filter_stream_with_callback(2, &cb as &dyn FilterCallback, &ctx)).await;
    let via_tx = collect_stream(tbl.filter_stream_with_callback_tx(
        None,
        2,
        &cb as &dyn FilterCallback,
        &ctx,
    ))
    .await;
    assert_eq!(baseline.len(), via_tx.len());
    assert_eq!(baseline.len(), 4);
}

/// KNOWN LIMITATION guard (C8): streaming scans do NOT overlay the tx's own
/// `write_set`, so a record staged inside a tx is INVISIBLE to an in-tx
/// stream until commit (only point reads — `read_one_tx` — do
/// read-your-own-writes). This test stages an insert and asserts the staged
/// record is ABSENT from `list_stream_tx(Some(&tx))`, pinning the *current*
/// documented behaviour.
///
/// When streaming read-your-own-writes is implemented, this test must FLIP:
/// the staged record becomes expected-present (and `list_stream_tx` should
/// then yield `n + 1` records). The assertions below will fail at that point,
/// forcing a deliberate update rather than letting the new behaviour ship as
/// a silent regression.
#[tokio::test]
async fn list_stream_tx_does_not_see_staged_insert() {
    let (tbl, ids) = make_table_with_n_records(3).await;
    let mut tx = make_tx(123);

    // Stage an insert inside the tx (populates tx.write_set for this table).
    let staged = tbl
        .insert_tx(&InnerValue::Str("staged-only".into()), Some(&mut tx))
        .await
        .unwrap();

    // The streamed scan forwards to the committed data store and does NOT
    // overlay staging, so it yields exactly the pre-staged committed set.
    let streamed = collect_stream(tbl.list_stream_tx(Some(&tx), 2)).await;

    assert_eq!(
        streamed.len(),
        ids.len(),
        "streaming scan must yield only committed records (no read-your-own-writes)"
    );
    assert!(
        streamed.iter().all(|(rid, _)| *rid != staged),
        "the staged-but-uncommitted insert must be ABSENT from the in-tx stream \
         (current limitation: scans do not overlay tx.write_set)"
    );
    // Cross-check via the point-read path, which DOES overlay staging: the
    // same record is visible there (read-your-own-writes), proving the
    // divergence is the streaming path's limitation, not a lost write.
    assert!(
        tbl.read_one_tx(staged, Some(&tx)).await.is_ok(),
        "read_one_tx must see the tx's own staged insert (point-read RYOW holds)"
    );
}

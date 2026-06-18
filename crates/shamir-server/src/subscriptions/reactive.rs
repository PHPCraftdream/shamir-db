use shamir_collections::{new_map, TMap};
use shamir_db::access::Actor;
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding, SubBatchOp,
};
use shamir_query_types::filter::FilterValue;
use shamir_tx::ChangeOp;

use super::payload::hex_encode;

/// Inject `$event.*` bindings into a cloned bind map.
fn inject_event_bindings(
    bind: &mut TMap<String, FilterValue>,
    change: &shamir_tx::changefeed::RecordChange,
    commit_version: u64,
) {
    let op_str = match change.op {
        ChangeOp::Put => "put",
        ChangeOp::Delete => "delete",
    };
    bind.insert(
        "$event.table".into(),
        FilterValue::String(change.table.clone()),
    );
    bind.insert("$event.op".into(), FilterValue::String(op_str.into()));
    bind.insert(
        "$event.key".into(),
        FilterValue::String(hex_encode(&change.key)),
    );
    bind.insert(
        "$event.commit_version".into(),
        FilterValue::Int(commit_version as i64),
    );
}

/// Build a wrapper `BatchRequest` that wraps a sub-batch with merged bindings,
/// execute it, and return the msgpack-encoded response (or error bytes).
pub(super) async fn execute_reactive_batch(
    db: &ShamirDb,
    db_name: &str,
    actor: &Actor,
    sub_batch: &SubBatchOp,
    change: &shamir_tx::changefeed::RecordChange,
    commit_version: u64,
) -> Vec<u8> {
    let mut merged_bind = sub_batch.bind.clone();
    inject_event_bindings(&mut merged_bind, change, commit_version);

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert(
        "_sub".into(),
        QueryEntry {
            op: BatchOp::Batch(SubBatchOp {
                batch: sub_batch.batch.clone(),
                bind: merged_bind,
            }),
            return_result: true,
            after: Vec::new(),
        },
    );

    let wrapper = BatchRequest {
        id: QueryValue::Null,
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    match db.execute_as(actor.clone(), db_name, &wrapper).await {
        Ok(response) => rmp_serde::to_vec_named(&response).unwrap_or_default(),
        Err(e) => {
            let mut m = TMap::default();
            m.insert("error".to_string(), QueryValue::Str(e.to_string()));
            rmp_serde::to_vec_named(&QueryValue::Map(m)).unwrap_or_default()
        }
    }
}

/// Build a wrapper `BatchRequest` that wraps a Call op with `$event.*`
/// bindings, execute it, and return the msgpack-encoded response.
pub(super) async fn execute_reactive_call(
    db: &ShamirDb,
    db_name: &str,
    actor: &Actor,
    call_op: &shamir_query_types::call::CallOp,
    change: &shamir_tx::changefeed::RecordChange,
    commit_version: u64,
) -> Vec<u8> {
    // Wrap the call inside a sub-batch so $event.* params are available
    // to the function via the bind map → FilterContext.params resolution.
    let mut bind: TMap<String, FilterValue> = new_map();
    inject_event_bindings(&mut bind, change, commit_version);

    // Inner batch with a single Call op.
    let mut inner_queries: TMap<String, QueryEntry> = new_map();
    inner_queries.insert(
        "_call".into(),
        QueryEntry {
            op: BatchOp::Call(call_op.clone()),
            return_result: true,
            after: Vec::new(),
        },
    );
    let inner_batch = BatchRequest {
        id: QueryValue::Null,
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let mut outer_queries: TMap<String, QueryEntry> = new_map();
    outer_queries.insert(
        "_sub".into(),
        QueryEntry {
            op: BatchOp::Batch(SubBatchOp {
                batch: inner_batch,
                bind,
            }),
            return_result: true,
            after: Vec::new(),
        },
    );
    let wrapper = BatchRequest {
        id: QueryValue::Null,
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    match db.execute_as(actor.clone(), db_name, &wrapper).await {
        Ok(response) => rmp_serde::to_vec_named(&response).unwrap_or_default(),
        Err(e) => {
            let mut m = TMap::default();
            m.insert("error".to_string(), QueryValue::Str(e.to_string()));
            rmp_serde::to_vec_named(&QueryValue::Map(m)).unwrap_or_default()
        }
    }
}

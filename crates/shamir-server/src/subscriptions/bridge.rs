use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use shamir_collections::{new_map, TMap};
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::server::conn_services::PushSink;
use shamir_db::access::Actor;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::batch::{BatchLimits, BatchOp, BatchRequest, QueryEntry, SubBatchOp};
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::subscribe::deliver_mode::DeliverMode;
use shamir_query_types::subscribe::event_mask::EventMask;
use shamir_query_types::subscribe::source::SubscriptionSource;
use shamir_tunables::instance_defaults::{JOURNAL_BACKFILL_LIMIT, SLOW_CONSUMER_THRESHOLD};
use shamir_tx::ChangeOp;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{StreamExt, StreamMap};

/// Bridge task: subscribes to the changefeed broadcast for the relevant
/// repos, filters events by table name and event mask, and pushes
/// `PushEnvelope` frames through the connection's `PushSink`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn bridge_task(
    sub_id: u64,
    db: Arc<ShamirDb>,
    db_name: String,
    sources: Vec<SubscriptionSource>,
    deliver: DeliverMode,
    actor: Actor,
    push: Arc<dyn PushSink>,
    from_version: Option<u64>,
    initial: bool,
) {
    let seq = AtomicU64::new(0);

    let run = async {
        let targets: Vec<(String, String, EventMask, Option<Filter>)> = sources
            .iter()
            .map(|s| {
                (
                    s.table.repo.clone(),
                    s.table.table.clone(),
                    s.events.clone(),
                    s.filter.clone(),
                )
            })
            .collect();

        // Gather unique repos.
        let repos: Vec<String> = {
            let mut r: Vec<String> = targets.iter().map(|(repo, _, _, _)| repo.clone()).collect();
            r.sort();
            r.dedup();
            r
        };

        // Subscribe to each repo's changefeed FIRST — before journal backfill —
        // so we don't miss events between journal read and live subscription.
        let mut receivers = Vec::new();
        for repo in &repos {
            match db.subscribe_changelog(&db_name, repo).await {
                Some(rx) => receivers.push(rx),
                None => {
                    tracing::warn!(sub_id, repo = %repo, "changefeed not available");
                    return;
                }
            }
        }

        let mut consecutive_push_failures: u32 = 0;
        let mut watermarks: HashMap<String, u64> = HashMap::new();

        // Journal backfill for from_version resume.
        if let Some(fv) = from_version {
            for repo in &repos {
                if let Some(jr) = db
                    .read_changelog_from_journal(&db_name, repo, fv, JOURNAL_BACKFILL_LIMIT)
                    .await
                {
                    if let Some(gap) = jr.gap_at {
                        let s = seq.fetch_add(1, Ordering::Relaxed);
                        let gap_env = PushEnvelope {
                            push: PushKind::Gap,
                            sub: sub_id,
                            seq: s,
                            data: None,
                            gap_at: Some(gap),
                        };
                        if let Ok(frame) = rmp_serde::to_vec_named(&gap_env) {
                            let _ = push.try_push(frame);
                        }
                    }
                    for event in &jr.events {
                        for change in &event.changes {
                            let value_json = match (&change.op, change.value.as_deref()) {
                                (ChangeOp::Put, Some(bytes)) => {
                                    db.decode_record_value_json(
                                        &db_name,
                                        repo,
                                        &change.table,
                                        bytes,
                                    )
                                    .await
                                }
                                _ => None,
                            };
                            if !matches_any(
                                &targets,
                                repo,
                                &change.table,
                                &change.op,
                                value_json.as_ref(),
                            ) {
                                continue;
                            }
                            let data = make_deliver_data(
                                &deliver,
                                &db,
                                &db_name,
                                &actor,
                                change,
                                value_json.as_ref(),
                                event.commit_version,
                            )
                            .await;
                            if !try_push_event(
                                &push,
                                sub_id,
                                &seq,
                                data,
                                &mut consecutive_push_failures,
                            ) {
                                return;
                            }
                        }
                        let wm = watermarks.entry(repo.clone()).or_insert(0);
                        if event.commit_version > *wm {
                            *wm = event.commit_version;
                        }
                    }
                }
            }
        }

        // Initial snapshot: read existing records for subscribed tables.
        if initial {
            for (target_repo, target_table, _mask, filter) in &targets {
                let mut query =
                    Query::with_repo(target_repo.as_str(), target_table.as_str()).build();
                if filter.is_some() {
                    query.r#where = filter.clone();
                }
                let mut batch = Batch::new();
                batch.query("_snapshot", query);
                batch.return_all();
                let find_req = batch.build();

                match db.execute_as(actor.clone(), &db_name, &find_req).await {
                    Ok(response) => {
                        for qr in response.results.values() {
                            for record in &qr.records {
                                let key_value = record
                                    .get("_id")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null);
                                let obj = serde_json::json!({
                                    "table": target_table,
                                    "op": "put",
                                    "key": key_value,
                                    "commit_version": 0,
                                    "value": record
                                });
                                let data = serde_json::to_vec(&obj).unwrap_or_default();
                                if !try_push_event(
                                    &push,
                                    sub_id,
                                    &seq,
                                    data,
                                    &mut consecutive_push_failures,
                                ) {
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(sub_id, table = %target_table, "initial snapshot failed: {e}");
                    }
                }
            }

            let s = seq.fetch_add(1, Ordering::Relaxed);
            let ready = PushEnvelope {
                push: PushKind::Ready,
                sub: sub_id,
                seq: s,
                data: None,
                gap_at: None,
            };
            if let Ok(frame) = rmp_serde::to_vec_named(&ready) {
                let _ = push.try_push(frame);
            }

            // Seed watermarks from current commit version to prevent live
            // duplicates of records already delivered in the snapshot.
            for repo in targets
                .iter()
                .map(|(r, _, _, _)| r.as_str())
                .collect::<std::collections::HashSet<_>>()
            {
                if let Some(v) = db.current_commit_version(&db_name, repo).await {
                    let wm = watermarks.entry(repo.to_string()).or_insert(0);
                    if v > *wm {
                        *wm = v;
                    }
                }
            }
        }

        let mut streams: StreamMap<String, BroadcastStream<_>> = StreamMap::new();
        for (repo, rx) in repos.into_iter().zip(receivers) {
            streams.insert(repo, BroadcastStream::new(rx));
        }

        while let Some((repo, item)) = streams.next().await {
            match item {
                Ok(event) => {
                    let wm = watermarks.entry(repo.clone()).or_insert(0);
                    if event.commit_version <= *wm {
                        continue;
                    }
                    *wm = event.commit_version;
                    for change in &event.changes {
                        // De-intern the Put value once: the changefeed
                        // ships records as msgpack with `u64` interned
                        // map keys (`InnerValue`), but `filter_matches_value`
                        // and `make_event_data` both consume string-keyed
                        // `serde_json::Value`. A direct `serde_json` decode
                        // of the raw bytes fails for any non-empty map.
                        let value_json = match (&change.op, change.value.as_deref()) {
                            (ChangeOp::Put, Some(bytes)) => {
                                db.decode_record_value_json(&db_name, &repo, &change.table, bytes)
                                    .await
                            }
                            _ => None,
                        };
                        if !matches_any(
                            &targets,
                            &repo,
                            &change.table,
                            &change.op,
                            value_json.as_ref(),
                        ) {
                            continue;
                        }
                        let data = make_deliver_data(
                            &deliver,
                            &db,
                            &db_name,
                            &actor,
                            change,
                            value_json.as_ref(),
                            event.commit_version,
                        )
                        .await;
                        if !try_push_event(
                            &push,
                            sub_id,
                            &seq,
                            data,
                            &mut consecutive_push_failures,
                        ) {
                            return;
                        }
                    }
                }
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                    tracing::warn!(sub_id, repo = %repo, lagged = n, "changefeed lagged");
                    let s = seq.fetch_add(1, Ordering::Relaxed);
                    let gap = PushEnvelope {
                        push: PushKind::Gap,
                        sub: sub_id,
                        seq: s,
                        data: None,
                        gap_at: None,
                    };
                    if let Ok(frame) = rmp_serde::to_vec_named(&gap) {
                        let _ = push.try_push(frame);
                    }
                }
            }
        }
    }; // end of async block

    run.await;

    // Always emit Closed frame (best-effort) on self-exit so the client
    // knows the subscription ended server-side.
    let s = seq.load(Ordering::Relaxed);
    let closed = PushEnvelope {
        push: PushKind::Closed,
        sub: sub_id,
        seq: s,
        data: None,
        gap_at: None,
    };
    if let Ok(frame) = rmp_serde::to_vec_named(&closed) {
        let _ = push.try_push(frame);
    }
}

/// Build deliver payload for a single change, dispatching on `DeliverMode`.
async fn make_deliver_data(
    deliver: &DeliverMode,
    db: &ShamirDb,
    db_name: &str,
    actor: &Actor,
    change: &shamir_tx::changefeed::RecordChange,
    value_json: Option<&serde_json::Value>,
    commit_version: u64,
) -> Vec<u8> {
    match deliver {
        DeliverMode::Records => make_event_data(change, value_json, commit_version),
        DeliverMode::Keys => make_keys_data(&change.table, &change.op, &change.key, commit_version),
        DeliverMode::Batch(sub_batch) => {
            execute_reactive_batch(db, db_name, actor, sub_batch, change, commit_version).await
        }
        DeliverMode::Call(call_op) => {
            execute_reactive_call(db, db_name, actor, call_op, change, commit_version).await
        }
    }
}

/// Serialize and push an Event envelope. Returns `false` when the slow-consumer
/// threshold is hit and the bridge should shut down.
///
/// Seq is only advanced on successful delivery so the client never observes
/// a seq hole from transient backpressure drops.
fn try_push_event(
    push: &Arc<dyn PushSink>,
    sub_id: u64,
    seq: &AtomicU64,
    data: Vec<u8>,
    consecutive_push_failures: &mut u32,
) -> bool {
    let s = seq.load(Ordering::Relaxed);
    let envelope = PushEnvelope {
        push: PushKind::Event,
        sub: sub_id,
        seq: s,
        data: Some(data),
        gap_at: None,
    };
    let frame = match rmp_serde::to_vec_named(&envelope) {
        Ok(b) => b,
        Err(_) => return true, // serialization error — skip, not fatal
    };
    if push.try_push(frame).is_err() {
        *consecutive_push_failures += 1;
        if *consecutive_push_failures >= SLOW_CONSUMER_THRESHOLD {
            let sc_seq = seq.fetch_add(1, Ordering::Relaxed);
            let sc = PushEnvelope {
                push: PushKind::SlowConsumer,
                sub: sub_id,
                seq: sc_seq,
                data: None,
                gap_at: None,
            };
            if let Ok(sc_frame) = rmp_serde::to_vec_named(&sc) {
                let _ = push.try_push(sc_frame);
            }
            tracing::warn!(sub_id, "slow consumer — closing subscription");
            return false;
        }
        tracing::debug!(sub_id, "push rejected, event dropped");
    } else {
        *consecutive_push_failures = 0;
        seq.fetch_add(1, Ordering::Relaxed);
    }
    true
}

pub(crate) fn matches_any(
    targets: &[(String, String, EventMask, Option<Filter>)],
    repo: &str,
    table: &str,
    op: &ChangeOp,
    value: Option<&serde_json::Value>,
) -> bool {
    targets
        .iter()
        .any(|(target_repo, target_table, mask, filter)| {
            if target_repo != repo || target_table != table || !mask_matches(mask, op) {
                return false;
            }
            match (filter, op) {
                (Some(f), ChangeOp::Put) => match value {
                    Some(v) => filter_matches_value(f, v),
                    None => {
                        tracing::warn!(
                            "subscription filter: de-intern decode failed for Put value, \
                             skipping event (fail-closed)"
                        );
                        false
                    }
                },
                _ => true,
            }
        })
}

pub(crate) fn filter_matches_value(filter: &Filter, value: &serde_json::Value) -> bool {
    match filter {
        Filter::Eq { field, value: fv } => resolve_field(value, field) == filter_value_to_json(fv),
        Filter::Ne { field, value: fv } => resolve_field(value, field) != filter_value_to_json(fv),
        Filter::Gt { field, value: fv } => {
            cmp_json(&resolve_field(value, field), &filter_value_to_json(fv))
                == Some(std::cmp::Ordering::Greater)
        }
        Filter::Gte { field, value: fv } => matches!(
            cmp_json(&resolve_field(value, field), &filter_value_to_json(fv)),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ),
        Filter::Lt { field, value: fv } => {
            cmp_json(&resolve_field(value, field), &filter_value_to_json(fv))
                == Some(std::cmp::Ordering::Less)
        }
        Filter::Lte { field, value: fv } => matches!(
            cmp_json(&resolve_field(value, field), &filter_value_to_json(fv)),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ),
        Filter::In { field, values } => {
            let resolved = resolve_field(value, field);
            values.iter().any(|v| resolved == filter_value_to_json(v))
        }
        Filter::NotIn { field, values } => {
            let resolved = resolve_field(value, field);
            !values.iter().any(|v| resolved == filter_value_to_json(v))
        }
        Filter::IsNull { field } => resolve_field(value, field).is_null(),
        Filter::IsNotNull { field } => !resolve_field(value, field).is_null(),
        Filter::Exists { field } => !matches!(resolve_field(value, field), serde_json::Value::Null),
        Filter::NotExists { field } => {
            matches!(resolve_field(value, field), serde_json::Value::Null)
        }
        Filter::And { filters } => filters.iter().all(|f| filter_matches_value(f, value)),
        Filter::Or { filters } => filters.iter().any(|f| filter_matches_value(f, value)),
        Filter::Not { filter: f } => !filter_matches_value(f, value),
        // Unsupported variants should be rejected at grant time; if one
        // slips through, fail-closed (do not deliver).
        _ => false,
    }
}

fn resolve_field(value: &serde_json::Value, path: &[String]) -> serde_json::Value {
    let mut current = value;
    for segment in path {
        match current.get(segment.as_str()) {
            Some(v) => current = v,
            None => return serde_json::Value::Null,
        }
    }
    current.clone()
}

fn filter_value_to_json(fv: &FilterValue) -> serde_json::Value {
    match fv {
        FilterValue::Null => serde_json::Value::Null,
        FilterValue::Bool(b) => serde_json::Value::Bool(*b),
        FilterValue::Int(i) => serde_json::json!(*i),
        FilterValue::Float(f) => serde_json::json!(*f),
        FilterValue::String(s) => serde_json::Value::String(s.clone()),
        FilterValue::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(filter_value_to_json).collect())
        }
        _ => serde_json::Value::Null,
    }
}

fn cmp_json(a: &serde_json::Value, b: &serde_json::Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            a.as_f64().partial_cmp(&b.as_f64())
        }
        (serde_json::Value::String(a), serde_json::Value::String(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

pub(crate) fn mask_matches(mask: &EventMask, op: &ChangeOp) -> bool {
    matches!(
        (mask, op),
        (EventMask::All, _)
            | (EventMask::Put, ChangeOp::Put)
            | (EventMask::Delete, ChangeOp::Delete)
    )
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn make_event_data(
    change: &shamir_tx::changefeed::RecordChange,
    value_json: Option<&serde_json::Value>,
    commit_version: u64,
) -> Vec<u8> {
    let op_str = match change.op {
        ChangeOp::Put => "put",
        ChangeOp::Delete => "delete",
    };
    let key_value = rmp_serde::from_slice::<serde_json::Value>(&change.key)
        .unwrap_or_else(|_| serde_json::Value::String(hex_encode(&change.key)));
    let mut obj = serde_json::json!({
        "table": change.table,
        "op": op_str,
        "key": key_value,
        "commit_version": commit_version
    });
    if let Some(val) = value_json {
        obj["value"] = val.clone();
    }
    serde_json::to_vec(&obj).unwrap_or_default()
}

fn make_keys_data(table: &str, op: &ChangeOp, key: &[u8], commit_version: u64) -> Vec<u8> {
    let op_str = match op {
        ChangeOp::Put => "put",
        ChangeOp::Delete => "delete",
    };
    let key_value = rmp_serde::from_slice::<serde_json::Value>(key)
        .unwrap_or_else(|_| serde_json::Value::String(hex_encode(key)));
    serde_json::to_vec(&serde_json::json!({
        "table": table,
        "op": op_str,
        "key": key_value,
        "commit_version": commit_version
    }))
    .unwrap_or_default()
}

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
/// execute it, and return the msgpack-encoded response (or JSON error).
async fn execute_reactive_batch(
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
        id: serde_json::Value::Null,
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };

    match db.execute_as(actor.clone(), db_name, &wrapper).await {
        Ok(response) => rmp_serde::to_vec_named(&response).unwrap_or_default(),
        Err(e) => {
            serde_json::to_vec(&serde_json::json!({"error": e.to_string()})).unwrap_or_default()
        }
    }
}

/// Build a wrapper `BatchRequest` that wraps a Call op with `$event.*`
/// bindings, execute it, and return the msgpack-encoded response.
async fn execute_reactive_call(
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
        id: serde_json::Value::Null,
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
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
        id: serde_json::Value::Null,
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
    };

    match db.execute_as(actor.clone(), db_name, &wrapper).await {
        Ok(response) => rmp_serde::to_vec_named(&response).unwrap_or_default(),
        Err(e) => {
            serde_json::to_vec(&serde_json::json!({"error": e.to_string()})).unwrap_or_default()
        }
    }
}

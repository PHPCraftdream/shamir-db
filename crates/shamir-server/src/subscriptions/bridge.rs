use std::sync::Arc;

use shamir_collections::TFxMap;
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::server::conn_services::PushSink;
use shamir_db::access::Actor;
use shamir_db::core::interner::Interner;
use shamir_db::record_view::{RecordRef, RecordView};
use shamir_db::types::value::{InnerValue, QueryValue};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::filter::Filter;
use shamir_query_types::subscribe::deliver_mode::DeliverMode;
use shamir_query_types::subscribe::event_mask::EventMask;
use shamir_query_types::subscribe::source::SubscriptionSource;
use shamir_tunables::instance_defaults::JOURNAL_BACKFILL_LIMIT;
use shamir_tx::ChangeOp;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::OnceCell;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{StreamExt, StreamMap};

use super::decode_cache::{cache_evict_up_to, cache_get, cache_insert, CachedBytes};
use super::deliver_cache::{deliver_cache_evict_up_to, deliver_cache_get, deliver_cache_insert};
use super::filter_eval::{bytes_to_arc, CompiledFilterSlot};
use super::push::{make_deliver_data, try_push_event};
use super::target_match::{any_target_interested_indexed, build_target_index, matches_any_indexed};
use arc_swap::ArcSwapOption;

/// Convert cached raw bytes + interner to a `QueryValue` for delivery.
///
/// Tries the zero-copy `RecordView` lens first; falls back to full
/// `InnerValue` decode for bare-scalar / non-map records.
fn bytes_to_query_value(bytes: &[u8], interner_cell: &OnceCell<Interner>) -> Option<QueryValue> {
    let interner = interner_cell.get()?;
    // Try RecordView (zero-copy lens) first.
    if let Ok(view) = RecordView::new(bytes) {
        // `to_query_value` returns QueryValue::Null on de-intern error;
        // for a valid map that's the best we can do — return it.
        return Some(view.to_query_value(interner));
    }
    // Fallback: full InnerValue decode for non-map records.
    let inner = InnerValue::from_bytes(bytes).ok()?;
    Some(inner.to_query_value(interner))
}

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
    // Unique per ShamirDb instance -- prevents deliver-cache pollution across
    // distinct in-memory databases in tests.
    let db_id = Arc::as_ptr(&db) as u64;
    // Decode/deliver cache discriminator: combines the instance pointer AND
    // the database NAME. The instance ptr alone is identical for every
    // database hosted by one ShamirDb (the production / e2e shape: one server,
    // many `createDb` databases), so a key without the db name lets two
    // databases that share a repo name ("main") and overlapping low
    // commit-versions collide in the GLOBAL cache — returning another db's
    // record bytes for filter evaluation. Hashing in `db_name` isolates them.
    let db_disc: u64 = {
        use std::hash::{BuildHasher, Hash, Hasher};
        let mut h = shamir_collections::THasher::default().build_hasher();
        db_id.hash(&mut h);
        db_name.hash(&mut h);
        h.finish()
    };

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

        // Subscribe to each repo's changefeed FIRST -- before journal backfill --
        // so we don't miss events between journal read and live subscription.
        let mut receivers = Vec::with_capacity(repos.len());
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
        // repo name -> index (built once; repos are fixed at subscribe time).
        let repo_idx: TFxMap<String, usize> = repos
            .iter()
            .enumerate()
            .map(|(i, r)| (r.clone(), i))
            .collect();
        // O(1) target index: (repo_idx, table) -> target indices.
        // Built once at subscribe time; replaces the two O(T) linear scans.
        let target_index = build_target_index(&targets, &repo_idx);

        // Per-target compiled-filter cache slots (parallel to `targets`).
        // `Some(slot)` for targets with a filter (lazily populated on first
        // event, recompiled when the interner grows); `None` for targets
        // without a filter. This avoids re-running `compile_filter` (which
        // includes `Regex::new`, `TSet` construction, `String` clones) on
        // every event × every subscriber.
        let filter_slots: Vec<Option<CompiledFilterSlot>> = targets
            .iter()
            .map(|(_, _, _, filter)| filter.as_ref().map(|_| ArcSwapOption::empty()))
            .collect();
        let mut watermarks: Vec<u64> = vec![0; repos.len()];

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
                            // O(1) pre-check via target index: skip the async
                            // interner lookup entirely when no target could match.
                            let ri = repo_idx[repo];
                            if !any_target_interested_indexed(
                                &targets,
                                &target_index,
                                ri,
                                &change.table,
                                &change.op,
                            ) {
                                continue;
                            }
                            let bytes_decoded = match (&change.op, change.value.as_deref()) {
                                (ChangeOp::Put, Some(bytes)) => db
                                    .get_table_interner_cell(&db_name, repo, &change.table)
                                    .await
                                    .map(|interner_cell| (bytes_to_arc(bytes), interner_cell)),
                                _ => None,
                            };
                            if !matches_any_indexed(
                                &targets,
                                &filter_slots,
                                &target_index,
                                ri,
                                &change.table,
                                &change.op,
                                bytes_decoded.as_ref(),
                            ) {
                                continue;
                            }
                            // Convert bytes -> QueryValue lazily: only for
                            // events that passed the filter and must be
                            // delivered. Mirrors the cached fan-out path: only
                            // `DeliverMode::Records` consumes value_qv (via
                            // `make_event_data`); Keys/Batch/Call ignore it, so
                            // gate the de-intern on Records to skip the decode
                            // entirely in those modes.
                            let value_qv: Option<QueryValue> =
                                if matches!(deliver, DeliverMode::Records) {
                                    match bytes_decoded.as_ref() {
                                        Some((bytes, cell)) => bytes_to_query_value(bytes, cell),
                                        None => None,
                                    }
                                } else {
                                    None
                                };
                            let data = make_deliver_data(
                                &deliver,
                                &db,
                                &db_name,
                                &actor,
                                change,
                                value_qv.as_ref(),
                                event.commit_version,
                            )
                            .await;
                            if !try_push_event(
                                &push,
                                sub_id,
                                &seq,
                                &data,
                                &mut consecutive_push_failures,
                            ) {
                                return;
                            }
                        }
                        let wm = &mut watermarks[repo_idx[repo]];
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
                                let record_val = record.as_value();
                                let key_qv: QueryValue =
                                    record_val.get("_id").cloned().unwrap_or(QueryValue::Null);
                                let value_qv: QueryValue = record_val.into_owned();
                                #[derive(serde::Serialize)]
                                struct SnapshotEvent<'x> {
                                    table: &'x str,
                                    op: &'x str,
                                    key: &'x QueryValue,
                                    value: &'x QueryValue,
                                    commit_version: u64,
                                }
                                let obj = SnapshotEvent {
                                    table: target_table,
                                    op: "put",
                                    key: &key_qv,
                                    value: &value_qv,
                                    commit_version: 0,
                                };
                                let data = rmp_serde::to_vec_named(&obj).unwrap_or_default();
                                if !try_push_event(
                                    &push,
                                    sub_id,
                                    &seq,
                                    &data,
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
            // Iterate unique repos via the pre-built index (no per-event alloc).
            for (repo, &idx) in &repo_idx {
                if let Some(v) = db.current_commit_version(&db_name, repo).await {
                    let wm = &mut watermarks[idx];
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
                    let wm = &mut watermarks[repo_idx[&repo]];
                    if event.commit_version <= *wm {
                        continue;
                    }
                    *wm = event.commit_version;
                    let ri = repo_idx[&repo];
                    for (change_idx, change) in event.changes.iter().enumerate() {
                        // O(1) pre-check via target index: skip the async
                        // interner lookup entirely when no target could match.
                        if !any_target_interested_indexed(
                            &targets,
                            &target_index,
                            ri,
                            &change.table,
                            &change.op,
                        ) {
                            continue;
                        }
                        // Cache raw bytes + interner per event across all
                        // bridge tasks: the cache deduplicates the interner
                        // lookup so that N subscribers pay O(1).
                        // No InnerValue decode on the filter path -- the
                        // RecordView lens reads fields zero-copy.
                        let decoded_arc: CachedBytes = match (&change.op, change.value.as_deref()) {
                            (ChangeOp::Put, Some(bytes)) => {
                                if let Some(cached) =
                                    cache_get(db_disc, &repo, event.commit_version, change_idx)
                                {
                                    cached
                                } else {
                                    let entry = db
                                        .get_table_interner_cell(&db_name, &repo, &change.table)
                                        .await
                                        .map(|interner_cell| (bytes_to_arc(bytes), interner_cell));
                                    cache_insert(
                                        db_disc,
                                        &repo,
                                        event.commit_version,
                                        change_idx,
                                        entry,
                                    )
                                }
                            }
                            _ => {
                                static NONE_ARC: std::sync::OnceLock<CachedBytes> =
                                    std::sync::OnceLock::new();
                                Arc::clone(NONE_ARC.get_or_init(|| Arc::new(None)))
                            }
                        };
                        let bytes_ref = (*decoded_arc).as_ref();
                        if !matches_any_indexed(
                            &targets,
                            &filter_slots,
                            &target_index,
                            ri,
                            &change.table,
                            &change.op,
                            bytes_ref,
                        ) {
                            continue;
                        }
                        // Lazy decode of the interner-encoded value into a
                        // QueryValue. Only `DeliverMode::Records` reads it
                        // (via `make_event_data`); Keys/Batch/Call never do.
                        // We therefore defer the de-intern until we know we're
                        // on a Records deliver-cache MISS -- on cache hits and
                        // in every other mode the decode is skipped entirely.
                        let deliver_mode_disc = match &deliver {
                            DeliverMode::Records => Some(0u8),
                            DeliverMode::Keys => Some(1u8),
                            _ => None,
                        };
                        // For Records/Keys the payload is shared via Arc --
                        // we borrow it for serialization (zero-copy fan-out).
                        // For Batch/Call the payload is built per-subscriber.
                        let cached_arc;
                        let owned_buf;
                        let data_ref: &[u8] = if let Some(mode) = deliver_mode_disc {
                            if let Some(arc) = deliver_cache_get(
                                db_disc,
                                &repo,
                                event.commit_version,
                                change_idx,
                                mode,
                            ) {
                                cached_arc = arc;
                                &cached_arc
                            } else {
                                // Cache MISS in the Records/Keys shared branch.
                                // Only Records actually consumes value_qv
                                // (`make_event_data`); Keys/Batch/Call ignore
                                // it, so gate the de-intern on Records (mode 0)
                                // to skip it entirely in Keys mode.
                                let value_qv: Option<QueryValue> = if mode == 0 {
                                    match bytes_ref {
                                        Some((bytes, cell)) => bytes_to_query_value(bytes, cell),
                                        None => None,
                                    }
                                } else {
                                    None
                                };
                                let built = make_deliver_data(
                                    &deliver,
                                    &db,
                                    &db_name,
                                    &actor,
                                    change,
                                    value_qv.as_ref(),
                                    event.commit_version,
                                )
                                .await;
                                cached_arc = deliver_cache_insert(
                                    db_disc,
                                    &repo,
                                    event.commit_version,
                                    change_idx,
                                    mode,
                                    built,
                                );
                                &cached_arc
                            }
                        } else {
                            // Batch/Call: `make_deliver_data` ignores the
                            // value_qv arg, so pass None and skip the decode.
                            owned_buf = make_deliver_data(
                                &deliver,
                                &db,
                                &db_name,
                                &actor,
                                change,
                                None,
                                event.commit_version,
                            )
                            .await;
                            &owned_buf
                        };
                        if !try_push_event(
                            &push,
                            sub_id,
                            &seq,
                            data_ref,
                            &mut consecutive_push_failures,
                        ) {
                            return;
                        }
                    }
                    // Evict stale cache entries -- safe because all bridges
                    // for this repo advance monotonically past this version.
                    if event.commit_version > 1 {
                        cache_evict_up_to(event.commit_version - 1);
                        deliver_cache_evict_up_to(event.commit_version - 1);
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

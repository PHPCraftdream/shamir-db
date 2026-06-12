use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use shamir_collections::THasher;

use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::server::conn_services::PushSink;
use shamir_db::access::Actor;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::filter::Filter;
use shamir_query_types::subscribe::deliver_mode::DeliverMode;
use shamir_query_types::subscribe::event_mask::EventMask;
use shamir_query_types::subscribe::source::SubscriptionSource;
use shamir_tunables::instance_defaults::JOURNAL_BACKFILL_LIMIT;
use shamir_tx::ChangeOp;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{StreamExt, StreamMap};

use super::decode_cache::{cache_evict_up_to, cache_get, cache_insert};
use super::deliver_cache::{deliver_cache_evict_up_to, deliver_cache_get, deliver_cache_insert};
use super::push::{make_deliver_data, try_push_event};
use super::target_match::{any_target_interested_indexed, build_target_index, matches_any_indexed};

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
    // Unique per ShamirDb instance — prevents deliver-cache pollution across
    // distinct in-memory databases in tests.
    let db_id = Arc::as_ptr(&db) as u64;

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
        // repo name → index (built once; repos are fixed at subscribe time).
        let repo_idx: HashMap<String, usize, THasher> = repos
            .iter()
            .enumerate()
            .map(|(i, r)| (r.clone(), i))
            .collect();
        // O(1) target index: (repo_idx, table) → target indices.
        // Built once at subscribe time; replaces the two O(T) linear scans.
        let target_index = build_target_index(&targets, &repo_idx);
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
                            // de-intern entirely when no target could match.
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
                            if !matches_any_indexed(
                                &targets,
                                &target_index,
                                ri,
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
                                let record_json = record.as_json();
                                let key_value = record_json
                                    .get("_id")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null);
                                let obj = serde_json::json!({
                                    "table": target_table,
                                    "op": "put",
                                    "key": key_value,
                                    "commit_version": 0,
                                    "value": *record_json
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
                        // de-intern entirely when no target could match.
                        if !any_target_interested_indexed(
                            &targets,
                            &target_index,
                            ri,
                            &change.table,
                            &change.op,
                        ) {
                            continue;
                        }
                        // De-intern the Put value once per event across
                        // all bridge tasks: the decode cache deduplicates
                        // the expensive msgpack+interner decode so that N
                        // subscribers pay O(1) instead of O(N).
                        let value_json_arc = match (&change.op, change.value.as_deref()) {
                            (ChangeOp::Put, Some(bytes)) => {
                                if let Some(cached) =
                                    cache_get(&repo, event.commit_version, change_idx)
                                {
                                    cached
                                } else {
                                    let decoded = db
                                        .decode_record_value_json(
                                            &db_name,
                                            &repo,
                                            &change.table,
                                            bytes,
                                        )
                                        .await;
                                    cache_insert(&repo, event.commit_version, change_idx, decoded)
                                }
                            }
                            _ => {
                                static NONE_ARC: std::sync::OnceLock<
                                    Arc<Option<serde_json::Value>>,
                                > = std::sync::OnceLock::new();
                                Arc::clone(NONE_ARC.get_or_init(|| Arc::new(None)))
                            }
                        };
                        let value_json: Option<&serde_json::Value> = (*value_json_arc).as_ref();
                        if !matches_any_indexed(
                            &targets,
                            &target_index,
                            ri,
                            &change.table,
                            &change.op,
                            value_json,
                        ) {
                            continue;
                        }
                        // For Records/Keys modes the payload is identical
                        // across all subscribers — use the deliver cache to
                        // build it once and share via Arc.
                        let deliver_mode_disc = match &deliver {
                            DeliverMode::Records => Some(0u8),
                            DeliverMode::Keys => Some(1u8),
                            _ => None,
                        };
                        let data = if let Some(mode) = deliver_mode_disc {
                            if let Some(cached) = deliver_cache_get(
                                db_id,
                                &repo,
                                event.commit_version,
                                change_idx,
                                mode,
                            ) {
                                (*cached).clone()
                            } else {
                                let built = make_deliver_data(
                                    &deliver,
                                    &db,
                                    &db_name,
                                    &actor,
                                    change,
                                    value_json,
                                    event.commit_version,
                                )
                                .await;
                                let arc = deliver_cache_insert(
                                    db_id,
                                    &repo,
                                    event.commit_version,
                                    change_idx,
                                    mode,
                                    built,
                                );
                                (*arc).clone()
                            }
                        } else {
                            make_deliver_data(
                                &deliver,
                                &db,
                                &db_name,
                                &actor,
                                change,
                                value_json,
                                event.commit_version,
                            )
                            .await
                        };
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
                    // Evict stale cache entries — safe because all bridges
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

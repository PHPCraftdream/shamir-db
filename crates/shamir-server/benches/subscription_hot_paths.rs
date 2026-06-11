//! Micro-benches for the Live Subscriptions v1.1 hot path — the functions
//! that execute on EVERY delivered change for EVERY active subscription.
//!
//! Measured: `target_match::matches_any` (per change × per target),
//! `filter_eval::filter_matches_value` (per filter eval), the de-intern
//! shim `ShamirDb::decode_record_value_json` (per Put change), and
//! `payload::make_event_data` (per delivered event). These add up to the
//! per-event server-side cost; baselining them in isolation lets us spot
//! regressions before they show up at the throughput level.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use serde_json::json;
use tokio::time::timeout;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;

use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::subscribe::EventMask;

use shamir_tx::changefeed::{ChangeOp, RecordChange};

use shamir_server::db_handler::{DbRequest, ShamirDbHandler};
use shamir_server::subscriptions::filter_eval::filter_matches_value;
use shamir_server::subscriptions::payload::make_event_data;
use shamir_server::subscriptions::target_match::matches_any;

// ────────────────────────── helpers ──────────────────────────

fn small_value() -> serde_json::Value {
    json!({
        "_id": "k1",
        "thread_id": 42,
        "status": "active",
        "body": "hello world",
        "ts": 1_700_000_000_i64,
    })
}

fn small_value_nonmatch() -> serde_json::Value {
    json!({
        "_id": "k2",
        "thread_id": 7,
        "status": "archived",
        "body": "nope",
        "ts": 1_700_000_001_i64,
    })
}

fn big_value(n: usize) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("_id".into(), json!("k1"));
    m.insert("thread_id".into(), json!(42));
    m.insert("status".into(), json!("active"));
    for i in 0..n.saturating_sub(3) {
        m.insert(format!("f{i}"), json!(i as i64));
    }
    serde_json::Value::Object(m)
}

fn nested_value() -> serde_json::Value {
    json!({
        "_id": "u1",
        "user": {
            "profile": {
                "name": "alice",
                "city": "Jerusalem",
            }
        },
        "thread_id": 42,
    })
}

// ────────────────────────── group 1: matches_any ──────────────────────────

fn bench_matches_any(c: &mut Criterion) {
    let mut g = c.benchmark_group("matches_any");
    g.throughput(Throughput::Elements(1));

    let put = ChangeOp::Put;

    // Targets, one per scenario. Each `Vec` is the same shape `bridge_task`
    // builds before entering its event loop.
    let mask_only: Vec<(String, String, EventMask, Option<Filter>)> =
        vec![("main".into(), "messages".into(), EventMask::All, None)];

    let eq_int: Vec<(String, String, EventMask, Option<Filter>)> = vec![(
        "main".into(),
        "messages".into(),
        EventMask::All,
        Some(Filter::Eq {
            field: vec!["thread_id".into()],
            value: FilterValue::Int(42),
        }),
    )];

    let and_two: Vec<(String, String, EventMask, Option<Filter>)> = vec![(
        "main".into(),
        "messages".into(),
        EventMask::All,
        Some(Filter::And {
            filters: vec![
                Filter::Eq {
                    field: vec!["thread_id".into()],
                    value: FilterValue::Int(42),
                },
                Filter::Eq {
                    field: vec!["status".into()],
                    value: FilterValue::String("active".into()),
                },
            ],
        }),
    )];

    let in_list_32: Vec<(String, String, EventMask, Option<Filter>)> = vec![(
        "main".into(),
        "messages".into(),
        EventMask::All,
        Some(Filter::In {
            field: vec!["thread_id".into()],
            values: (0i64..32).map(FilterValue::Int).collect(),
        }),
    )];
    // Make sure thread_id=42 is NOT in [0..32) so we can probe miss-on-value.
    // Hit case uses thread_id=10.
    let val_hit_in = json!({
        "_id": "k",
        "thread_id": 10,
        "status": "active",
        "body": "x",
    });

    let v_match = small_value();
    let v_miss = small_value_nonmatch();

    g.bench_function("mask_only_hit", |b| {
        b.iter(|| {
            black_box(matches_any(
                black_box(&mask_only),
                "main",
                "messages",
                &put,
                Some(black_box(&v_match)),
            ))
        });
    });

    g.bench_function("eq_int_hit", |b| {
        b.iter(|| {
            black_box(matches_any(
                black_box(&eq_int),
                "main",
                "messages",
                &put,
                Some(black_box(&v_match)),
            ))
        });
    });
    g.bench_function("eq_int_miss", |b| {
        b.iter(|| {
            black_box(matches_any(
                black_box(&eq_int),
                "main",
                "messages",
                &put,
                Some(black_box(&v_miss)),
            ))
        });
    });

    g.bench_function("and_two_hit", |b| {
        b.iter(|| {
            black_box(matches_any(
                black_box(&and_two),
                "main",
                "messages",
                &put,
                Some(black_box(&v_match)),
            ))
        });
    });
    g.bench_function("and_two_miss", |b| {
        b.iter(|| {
            black_box(matches_any(
                black_box(&and_two),
                "main",
                "messages",
                &put,
                Some(black_box(&v_miss)),
            ))
        });
    });

    g.bench_function("in_list_32_hit", |b| {
        b.iter(|| {
            black_box(matches_any(
                black_box(&in_list_32),
                "main",
                "messages",
                &put,
                Some(black_box(&val_hit_in)),
            ))
        });
    });
    g.bench_function("in_list_32_miss", |b| {
        b.iter(|| {
            black_box(matches_any(
                black_box(&in_list_32),
                "main",
                "messages",
                &put,
                Some(black_box(&v_match)), // thread_id=42 not in [0..32)
            ))
        });
    });

    g.finish();
}

// ────────────────────────── group 2: filter_matches_value ──────────────────────────

fn bench_filter_matches_value(c: &mut Criterion) {
    let mut g = c.benchmark_group("filter_matches_value");
    g.throughput(Throughput::Elements(1));

    let v = small_value();
    let vn = nested_value();

    let eq_int = Filter::Eq {
        field: vec!["thread_id".into()],
        value: FilterValue::Int(42),
    };
    g.bench_function("eq_int_top_level", |b| {
        b.iter(|| black_box(filter_matches_value(black_box(&eq_int), black_box(&v))));
    });

    let eq_str_nested = Filter::Eq {
        field: vec!["user".into(), "profile".into(), "name".into()],
        value: FilterValue::String("alice".into()),
    };
    g.bench_function("eq_str_nested_path", |b| {
        b.iter(|| {
            black_box(filter_matches_value(
                black_box(&eq_str_nested),
                black_box(&vn),
            ))
        });
    });

    let and_2 = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["thread_id".into()],
                value: FilterValue::Int(42),
            },
            Filter::Eq {
                field: vec!["status".into()],
                value: FilterValue::String("active".into()),
            },
        ],
    };
    g.bench_function("compound_and_2", |b| {
        b.iter(|| black_box(filter_matches_value(black_box(&and_2), black_box(&v))));
    });

    let and_3 = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["thread_id".into()],
                value: FilterValue::Int(42),
            },
            Filter::Eq {
                field: vec!["status".into()],
                value: FilterValue::String("active".into()),
            },
            Filter::Eq {
                field: vec!["body".into()],
                value: FilterValue::String("hello world".into()),
            },
        ],
    };
    g.bench_function("compound_and_3", |b| {
        b.iter(|| black_box(filter_matches_value(black_box(&and_3), black_box(&v))));
    });

    let or_2 = Filter::Or {
        filters: vec![
            Filter::Eq {
                field: vec!["thread_id".into()],
                value: FilterValue::Int(7),
            },
            Filter::Eq {
                field: vec!["thread_id".into()],
                value: FilterValue::Int(42),
            },
        ],
    };
    g.bench_function("compound_or_2", |b| {
        b.iter(|| black_box(filter_matches_value(black_box(&or_2), black_box(&v))));
    });

    let in_int_32 = Filter::In {
        field: vec!["thread_id".into()],
        values: (0i64..32).map(FilterValue::Int).collect(),
    };
    let val_in = json!({"thread_id": 10, "status": "active"});
    g.bench_function("in_int_list_32", |b| {
        b.iter(|| {
            black_box(filter_matches_value(
                black_box(&in_int_32),
                black_box(&val_in),
            ))
        });
    });

    let is_null = Filter::IsNull {
        field: vec!["missing".into()],
    };
    g.bench_function("is_null", |b| {
        b.iter(|| black_box(filter_matches_value(black_box(&is_null), black_box(&v))));
    });

    let exists = Filter::Exists {
        field: vec!["thread_id".into()],
    };
    g.bench_function("exists", |b| {
        b.iter(|| black_box(filter_matches_value(black_box(&exists), black_box(&v))));
    });

    let not_compound = Filter::Not {
        filter: Box::new(Filter::And {
            filters: vec![
                Filter::Eq {
                    field: vec!["thread_id".into()],
                    value: FilterValue::Int(7),
                },
                Filter::Eq {
                    field: vec!["status".into()],
                    value: FilterValue::String("archived".into()),
                },
            ],
        }),
    };
    g.bench_function("not_compound", |b| {
        b.iter(|| {
            black_box(filter_matches_value(
                black_box(&not_compound),
                black_box(&v),
            ))
        });
    });

    g.finish();
}

// ────────────── helpers for groups 3/4 ──────────────

fn fixture_session() -> Session {
    Session::new(
        [0x01u8; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0x77u8; 32],
        1_000_000,
    )
}

/// Spin up a ShamirDb, insert a record with `n_fields` payload fields,
/// capture its msgpack `RecordChange.value` bytes via the changefeed.
/// Returns `(db, db_name, repo, table, value_bytes)`.
///
/// We use option (a) from the task: capture real bytes by subscribing to
/// the changefeed. Constructing them by hand via `inner_to_msgpack` would
/// require touching the table's interner directly — going through the
/// real write path is simpler and guarantees realistic shape.
async fn build_db_with_change_bytes(
    n_fields: usize,
) -> (Arc<ShamirDb>, String, String, String, Bytes) {
    let db_name = "bench_db".to_string();
    let repo = "main".to_string();
    let table = "messages".to_string();

    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db(&db_name).await;
    let cfg =
        RepoConfig::new(&repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new(&table));
    shamir.add_repo(&db_name, cfg).await.unwrap();
    let shamir = Arc::new(shamir);

    // Tap the changefeed BEFORE the insert.
    let mut rx = shamir
        .subscribe_changelog(&db_name, &repo)
        .await
        .expect("subscribe_changelog");

    // Build the row payload with `n_fields` fields.
    let mut d = doc! { "_id" => "k1" };
    d = d.set("thread_id", 42_i64);
    for i in 0..n_fields.saturating_sub(2) {
        d = d.set(format!("f{i}"), i as i64);
    }
    let mut batch = Batch::new();
    batch.id(1);
    batch.insert("ins", insert(&table).row(d));

    // Drive through the handler (same path as production wire traffic).
    let handler = ShamirDbHandler::new(shamir.clone());
    let session = fixture_session();
    let req = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db_name.clone(),
        batch: batch.build(),
    };
    let bytes = rmp_serde::to_vec_named(&req).unwrap();
    handler
        .handle(&session, &bytes, &ConnectionServices::without_push(0))
        .await
        .unwrap();

    // Pull the changefeed event and grab the first Put's value bytes.
    let evt = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("changefeed recv timeout")
        .expect("changefeed broadcast closed");
    let value_bytes = evt
        .changes
        .iter()
        .find_map(|c| {
            if matches!(c.op, ChangeOp::Put) {
                c.value.clone()
            } else {
                None
            }
        })
        .expect("no Put change in event");

    (shamir, db_name, repo, table, value_bytes)
}

// ────────────────────────── group 3: decode_record_value_json ──────────────────────────

fn bench_decode_record_value_json(c: &mut Criterion) {
    let mut g = c.benchmark_group("decode_record_value_json");
    g.throughput(Throughput::Elements(1));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();

    for &n in &[5usize, 20] {
        let (db, db_name, repo, table, bytes) = rt.block_on(build_db_with_change_bytes(n));
        let bench_name = format!("fields_{n}");
        g.bench_function(&bench_name, |b| {
            b.iter(|| {
                let v = rt.block_on(db.decode_record_value_json(
                    black_box(&db_name),
                    black_box(&repo),
                    black_box(&table),
                    black_box(&bytes),
                ));
                black_box(v);
            });
        });
    }

    g.finish();
}

// ────────────────────────── group 4: make_event_data ──────────────────────────

fn bench_make_event_data(c: &mut Criterion) {
    let mut g = c.benchmark_group("make_event_data");
    g.throughput(Throughput::Elements(1));

    // Key bytes — msgpack-encoded `serde_json::Value::String("k1")`.
    let key_value = json!("k1");
    let key_bytes = rmp_serde::to_vec(&key_value).unwrap();

    let put_small = RecordChange {
        table: "messages".into(),
        key: Bytes::from(key_bytes.clone()),
        op: ChangeOp::Put,
        value: Some(Bytes::new()), // contents ignored; payload uses passed-in value_json
    };
    let v_small = small_value();
    g.bench_function("put_small_value", |b| {
        b.iter(|| {
            black_box(make_event_data(
                black_box(&put_small),
                Some(black_box(&v_small)),
                42,
            ))
        });
    });

    let v_big = big_value(20);
    g.bench_function("put_large_value", |b| {
        b.iter(|| {
            black_box(make_event_data(
                black_box(&put_small),
                Some(black_box(&v_big)),
                42,
            ))
        });
    });

    let del = RecordChange {
        table: "messages".into(),
        key: Bytes::from(key_bytes),
        op: ChangeOp::Delete,
        value: None,
    };
    g.bench_function("delete_no_value", |b| {
        b.iter(|| black_box(make_event_data(black_box(&del), None, 42)));
    });

    g.finish();
}

criterion_group!(
    benches,
    bench_matches_any,
    bench_filter_matches_value,
    bench_decode_record_value_json,
    bench_make_event_data
);
criterion_main!(benches);

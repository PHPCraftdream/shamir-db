//! SELECT projection bench — no GROUP BY, no aggregates.
//!
//! `SelectProjection::project_value` is called once per record on every read
//! query. Hot loop allocates:
//!   - `resolve_field` clones the leaf (already optimised on the
//!     filter side via `resolve_field_ref` — projection still uses
//!     the owned variant);
//!   - `inner_value_to_query_value` walks the leaf into QueryValue;
//!   - `key.to_string()` allocates the output map key per field
//!     per record (alias or last path segment).
//!
//! Bench drives `apply_select_value` over 1000 records, 5 selected fields.
//!
//! Note: J1 migration — apply_select (legacy value path) removed; bench now uses
//! apply_select_value (QueryValue path) + apply_order_by_qv.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): sync
//! fixtures (interner, records, select/order-by/pagination shapes) built
//! ONCE outside the timed closures (plan 1). The end-to-end push-down
//! comparison (`bench_pushdown`) builds a fresh `TableManager` per N
//! outside the timed loop too and drives async reads through
//! `bench_async` (harness-owned shared runtime).

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::filter::eval_context::FilterContext;
use shamir_engine::query::read::exec::{apply_order_by_qv, apply_pagination, apply_select_value};
use shamir_engine::query::read::{
    OrderBy, OrderByItem, OrderDirection, Pagination, ReadQuery, Select, SelectItem,
};
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

fn make_record(interner: &Interner, idx: u32) -> InnerValue {
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let mut m = new_map_wc(10);
    m.insert(touch("id"), InnerValue::Int(idx as i64));
    m.insert(touch("name"), InnerValue::Str(format!("user-{}", idx)));
    m.insert(touch("age"), InnerValue::Int((idx % 100) as i64));
    m.insert(touch("score"), InnerValue::F64(idx as f64 * 1.5));
    m.insert(
        touch("email"),
        InnerValue::Str(format!("u{}@example.com", idx)),
    );
    m.insert(touch("city"), InnerValue::Str("Jerusalem".into()));
    m.insert(touch("active"), InnerValue::Bool(idx.is_multiple_of(2)));
    InnerValue::Map(m)
}

async fn build_table_with_n(n: usize) -> shamir_engine::table::TableManager {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new(
        "bench".into(),
        BoxRepo::InMemory(repo),
        vec![shamir_engine::table::TableConfig::new("rows".to_string())],
    );
    let table = instance.get_table("rows").await.unwrap();
    let interner = table.interner().get().await.unwrap();

    // Pre-intern the field names so we can build interned records directly.
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let k_id = touch("id");
    let k_name = touch("name");
    let k_age = touch("age");
    let k_score = touch("score");
    let k_email = touch("email");
    let k_city = touch("city");
    let k_status = touch("status");
    let v_jerusalem = InnerValue::Str("Jerusalem".into());
    let v_active = InnerValue::Str("active".into());

    for i in 0..n {
        let mut m = new_map_wc(10);
        m.insert(k_id.clone(), InnerValue::Int(i as i64));
        m.insert(k_name.clone(), InnerValue::Str(format!("user-{}", i)));
        m.insert(k_age.clone(), InnerValue::Int((i % 100) as i64));
        m.insert(k_score.clone(), InnerValue::F64(i as f64 * 1.5));
        m.insert(
            k_email.clone(),
            InnerValue::Str(format!("u{}@example.com", i)),
        );
        m.insert(k_city.clone(), v_jerusalem.clone());
        // status = "active" for every row → WHERE matches all N
        m.insert(k_status.clone(), v_active.clone());
        table.insert(&InnerValue::Map(m)).await.unwrap();
    }

    table.create_index("status_idx", &["status"]).await.unwrap();
    table
}

fn main() {
    let mut h = Harness::new("select_pipeline", env!("CARGO_MANIFEST_DIR"));

    // ── Setup ────────────────────────────────────────────────────
    let interner: &'static Interner = Box::leak(Box::new(Interner::new()));
    for k in ["id", "name", "age", "score", "email", "city", "active"] {
        let _ = interner.touch_ind(k);
    }
    let records: Vec<(RecordId, InnerValue)> = (0..1000)
        .map(|i| (RecordId::new(), make_record(interner, i)))
        .collect();

    let select_5 = Select {
        items: vec![
            SelectItem::Field {
                path: vec!["id".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["name".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["age".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["score".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["email".to_string()],
                alias: None,
            },
        ],
        distinct: false,
    };

    let select_all = Select {
        items: vec![SelectItem::All],
        distinct: false,
    };

    {
        let records = records.clone();
        let select_5 = select_5.clone();
        h.bench("apply_select_value/5_fields_1000_records", move || {
            black_box(apply_select_value(&records, &select_5, interner));
        });
    }
    {
        let records = records.clone();
        let select_all = select_all.clone();
        h.bench("apply_select_value/select_all_1000_records", move || {
            black_box(apply_select_value(&records, &select_all, interner));
        });
    }

    // Projected QueryValues for ORDER BY bench. Build once, clone per
    // iteration so the sort is the only measured work.
    let projected: Vec<QueryValue> = apply_select_value(&records, &select_5, interner);
    let order_by_single = OrderBy {
        items: vec![OrderByItem {
            field: vec!["age".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };
    let order_by_two = OrderBy {
        items: vec![
            OrderByItem {
                field: vec!["age".to_string()],
                direction: OrderDirection::Asc,
                nulls: None,
            },
            OrderByItem {
                field: vec!["name".to_string()],
                direction: OrderDirection::Asc,
                nulls: None,
            },
        ],
    };

    // `apply_order_by_qv` sorts in place — a shared `projected` would be
    // mutated by the first iteration and stay sorted for the rest, so a
    // fresh clone is required every iteration (`bench_batched`, setup
    // untimed).
    {
        let projected = projected.clone();
        let order_by_single = order_by_single.clone();
        h.bench_batched(
            "apply_order_by_qv/single_int_1000",
            move || projected.clone(),
            move |mut recs| {
                apply_order_by_qv(&mut recs, &order_by_single);
                black_box(recs);
            },
        );
    }
    {
        let projected = projected.clone();
        let order_by_two = order_by_two.clone();
        h.bench_batched(
            "apply_order_by_qv/two_fields_1000",
            move || projected.clone(),
            move |mut recs| {
                apply_order_by_qv(&mut recs, &order_by_two);
                black_box(recs);
            },
        );
    }

    // ── apply_pagination ────────────────────────────────────────
    // `apply_pagination` consumes its `Vec<QueryValue>` by value, so a
    // fresh clone per iteration is required here too.
    {
        let projected = projected.clone();
        h.bench_batched(
            "apply_pagination/skip_50_limit_100",
            move || projected.clone(),
            move |recs| {
                black_box(apply_pagination(
                    recs,
                    &Pagination::LimitOffset {
                        limit: Some(100),
                        offset: 50,
                    },
                    false,
                ));
            },
        );
    }
    {
        let projected = projected.clone();
        h.bench_batched(
            "apply_pagination/limit_10_from_1000",
            move || projected.clone(),
            move |recs| {
                black_box(apply_pagination(
                    recs,
                    &Pagination::LimitOffset {
                        limit: Some(10),
                        offset: 0,
                    },
                    false,
                ));
            },
        );
    }
    {
        let projected = projected.clone();
        h.bench_batched(
            "apply_pagination/count_total_1000",
            move || projected.clone(),
            move |recs| {
                black_box(apply_pagination(recs, &Pagination::None, true));
            },
        );
    }

    // ============================================================================
    // Opt #3a — end-to-end LIMIT push-down bench.
    //
    // Setup: a real `TableManager` (in-memory) with N records, all matching
    // the WHERE clause. Bench two shapes of the SAME page-of-10 query:
    //
    //   `pushdown_active` — plain `WHERE … LIMIT 10` (no ORDER BY / DISTINCT /
    //       GROUP BY / aggregates). With Opt #3a the read pipeline projects
    //       only the 10 page rows.
    //   `pushdown_disabled` — same WHERE / same LIMIT 10, but with
    //       `ORDER BY name ASC` added. ORDER BY disables the gate, so the
    //       pipeline must project every match before sorting + truncating —
    //       this is the "before" baseline for the push-down.
    //
    // Both shapes touch the same N records / same index plan; the delta is
    // the projection cost of `N - 10` rows. The ratio shows the asymptotic
    // win as N grows.
    //
    // Note: ORDER BY here is NOT served by a sorted index (none defined on
    // `name`), so the dispatcher uses `read_collecting`, NOT the
    // `order_limit_fast_path`. This isolates the projection cost the
    // push-down skips.
    // ============================================================================
    let setup_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    for &n in &[10_000usize, 100_000usize] {
        let table = setup_rt.block_on(build_table_with_n(n));

        // (A) push-down active: WHERE + LIMIT 10, no ORDER BY.
        let q_pushdown: ReadQuery = {
            use shamir_types::mpack;
            let raw = mpack!({
                "from": "rows",
                "where": {"op": "eq", "field": ["status"], "value": "active"},
                "select": {
                    "items": [
                        {"type": "field", "path": ["name"]},
                        {"type": "field", "path": ["age"]},
                        {"type": "field", "path": ["score"]},
                        {"type": "field", "path": ["email"]}
                    ],
                    "distinct": false
                },
                "pagination": {"mode": "LimitOffset", "limit": 10}
            });
            let bytes = rmp_serde::to_vec_named(&raw).unwrap();
            rmp_serde::from_slice(&bytes).unwrap()
        };

        // (B) push-down disabled: same shape + ORDER BY name (no sorted
        //     index → falls to `read_collecting` which projects every
        //     match before sorting + truncating).
        let q_full: ReadQuery = {
            use shamir_types::mpack;
            let raw = mpack!({
                "from": "rows",
                "where": {"op": "eq", "field": ["status"], "value": "active"},
                "select": {
                    "items": [
                        {"type": "field", "path": ["name"]},
                        {"type": "field", "path": ["age"]},
                        {"type": "field", "path": ["score"]},
                        {"type": "field", "path": ["email"]}
                    ],
                    "distinct": false
                },
                "order_by": {"items": [{"field": ["name"], "direction": "asc"}]},
                "pagination": {"mode": "LimitOffset", "limit": 10}
            });
            let bytes = rmp_serde::to_vec_named(&raw).unwrap();
            rmp_serde::from_slice(&bytes).unwrap()
        };

        {
            let table = table.clone();
            let q = q_pushdown.clone();
            h.bench_async(&format!("read_limit_pushdown/pushdown_active_N={n}"), move || {
                let table = table.clone();
                let q = q.clone();
                async move {
                    let interner = table.interner().get().await.unwrap();
                    let refs = new_map();
                    let ctx = FilterContext::new(interner, &refs);
                    let r = table.read(&q, &ctx).await.unwrap();
                    black_box(r);
                }
            });
        }

        {
            let table = table.clone();
            let q = q_full.clone();
            h.bench_async(
                &format!("read_limit_pushdown/pushdown_disabled_N={n}"),
                move || {
                    let table = table.clone();
                    let q = q.clone();
                    async move {
                        let interner = table.interner().get().await.unwrap();
                        let refs = new_map();
                        let ctx = FilterContext::new(interner, &refs);
                        let r = table.read(&q, &ctx).await.unwrap();
                        black_box(r);
                    }
                },
            );
        }
    }

    h.run();
}

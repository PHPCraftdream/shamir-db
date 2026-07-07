//! ORDER BY baseline benchmark.
//!
//! Measures the three realistic ORDER BY scenarios and quantifies how
//! much CPU is spent in field resolution vs the sort + permutation itself.
//!
//! Originally measured at 100k records (release build, 5 fields):
//!   - order_by_indexed_field_limit_100  (score, sorted): ~84 ms / iter
//!   - order_by_non_indexed_field_limit_100 (email):     ~107 ms / iter
//!   - order_by_non_indexed_field_full (email):           ~94 ms / iter
//!
//! Detailed phase breakdown via `examples/prof_order_by.rs`
//! (`cargo run --release --example prof_order_by`):
//!   - Pure `Vec<QueryValue>` permutation:          ~1.3 ms
//!   - Pre-extracted sort + permute (no lookup):   ~5.3 ms
//!   - Full apply_order_by (with comparator lookup): ~35 ms
//!   - Lookup + value-swap overhead:               ~30 ms (= 85% of sort)
//!
//! Verdict: `Value::get` field lookup inside the comparator is the
//! dominant cost — ~85% of ORDER BY time, 17% of the whole read
//! pipeline. Pre-extracting sort keys collapses it by ~6.7×.
//!
//! Conclusion: PROCEED with task #67 (precomputed field positions /
//! pre-extracted sort keys).
//!
//! After #67 (commit pending, typed-SortKey enum + index sort):
//!   - apply_order_by (email): ~37 ms (was ~44 ms) — ~15-20% wall-clock win
//!   - Theoretical floor from prof_order_by remains ~25 ms
//!     (5 ms sort+permute + 20 ms extract); the gap is enum-tag matching
//!     and SmallVec[SortKey; 4] cache pressure. A typed columnar buffer
//!     for the single-column case would unlock the remaining ~6× — tracked
//!     as a follow-up refinement (do after #70 / arena lands).
//!
//! NOTE: the fixture N was 100_000 during the original Criterion
//! measurements above. It has been lowered to 2_000 to fit the
//! fixed-iteration harness's per-call budget (the comparator-lookup cost
//! is linear in N, and each timed iteration also pays an untimed
//! `projected.clone()` of N QueryValue maps; at 100k the combined cost
//! was ~50-100ms/call). The relative phase breakdown is unchanged; only
//! the absolute scale shrinks.
//!
//! Note: J1 migration — `apply_select` removed; this bench now uses
//! `apply_select_value` + `apply_order_by_qv` (QueryValue path).
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): the
//! projected `Vec<QueryValue>` fixture is built ONCE outside the timed
//! closures. `apply_order_by_qv` sorts in place (mutates its input), so a
//! shared instance can't be reused across iterations — every scenario uses
//! `bench_batched` with an untimed `projected.clone()` setup, matching the
//! original Criterion `b.iter_batched(|| projected.clone(), ..., SmallInput)`.

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_engine::query::read::exec::{apply_order_by_qv, apply_pagination, apply_select_value};
use shamir_engine::query::read::{
    OrderBy, OrderByItem, OrderDirection, Pagination, Select, SelectItem,
};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

fn touch(interner: &Interner, s: &str) -> InternerKey {
    match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    }
}

/// Build one record: `{ id, name, email, score, active, created_at }`.
fn make_record(interner: &Interner, idx: u32) -> InnerValue {
    let mut m = new_map_wc(8);
    m.insert(touch(interner, "id"), InnerValue::Int(idx as i64));
    m.insert(
        touch(interner, "name"),
        InnerValue::Str(format!("user-{idx}")),
    );
    m.insert(
        touch(interner, "email"),
        InnerValue::Str(format!("user{idx}@example.com")),
    );
    m.insert(
        touch(interner, "score"),
        InnerValue::F64((idx as f64) * 1.7),
    );
    m.insert(
        touch(interner, "active"),
        InnerValue::Bool(idx.is_multiple_of(2)),
    );
    m.insert(
        touch(interner, "created_at"),
        InnerValue::Int(1_700_000_000 + idx as i64),
    );
    InnerValue::Map(m)
}

fn main() {
    let mut h = Harness::new("order_by_pipeline", env!("CARGO_MANIFEST_DIR"));

    // ── Setup ─────────────────────────────────────────────────────
    let interner = Interner::new();
    for k in ["id", "name", "email", "score", "active", "created_at"] {
        let _ = interner.touch_ind(k);
    }

    // 2_000 (was 100_000): see the module-level NOTE — the comparator-
    // lookup cost is linear in N, and every iteration also pays an
    // untimed `projected.clone()` (O(N) QueryValue map clones) before the
    // timed sort. At 100k the combined cost was ~50-100ms/call; 2_000
    // keeps the timed sort under the ~10ms/call budget while preserving
    // the relative phase breakdown.
    let n_records: u64 = 2_000;
    let raw_records: Vec<(RecordId, InnerValue)> = (0..n_records)
        .map(|i| (RecordId::new(), make_record(&interner, i as u32)))
        .collect();

    let select_all = Select {
        items: vec![SelectItem::All],
        distinct: false,
    };

    // Project once — ORDER BY operates on `Vec<QueryValue>`.
    let projected: Vec<QueryValue> = apply_select_value(&raw_records, &select_all, &interner);

    let order_by_score = OrderBy {
        items: vec![OrderByItem {
            field: vec!["score".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };

    let order_by_email = OrderBy {
        items: vec![OrderByItem {
            field: vec!["email".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };

    // ── Scenario 1: indexed field + LIMIT 100 ─────────────────────
    {
        let projected = projected.clone();
        let order_by_score = order_by_score.clone();
        h.bench_batched(
            "order_by_indexed_field_limit_100/score_asc_limit_100",
            move || projected.clone(),
            move |mut recs| {
                apply_order_by_qv(&mut recs, &order_by_score);
                let limited = apply_pagination(
                    recs,
                    &Pagination::LimitOffset {
                        limit: Some(100),
                        offset: 0,
                    },
                    false,
                );
                black_box(limited);
            },
        );
    }

    // ── Scenario 2: non-indexed field + LIMIT 100 ─────────────────
    {
        let projected = projected.clone();
        let order_by_email = order_by_email.clone();
        h.bench_batched(
            "order_by_non_indexed_field_limit_100/email_asc_limit_100",
            move || projected.clone(),
            move |mut recs| {
                apply_order_by_qv(&mut recs, &order_by_email);
                let limited = apply_pagination(
                    recs,
                    &Pagination::LimitOffset {
                        limit: Some(100),
                        offset: 0,
                    },
                    false,
                );
                black_box(limited);
            },
        );
    }

    // ── Scenario 3: non-indexed field, full sort (no LIMIT) ───────
    {
        let projected = projected.clone();
        let order_by_email = order_by_email.clone();
        h.bench_batched(
            "order_by_non_indexed_field_full/email_asc_full",
            move || projected.clone(),
            move |mut recs| {
                apply_order_by_qv(&mut recs, &order_by_email);
                black_box(recs);
            },
        );
    }

    // ══════════════════════════════════════════════════════════════
    // Single-column type-specialised scenarios (for #109)
    // ══════════════════════════════════════════════════════════════

    let order_by_id = OrderBy {
        items: vec![OrderByItem {
            field: vec!["id".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };
    let order_by_active = OrderBy {
        items: vec![OrderByItem {
            field: vec!["active".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };
    let order_by_multi = OrderBy {
        items: vec![
            OrderByItem {
                field: vec!["active".to_string()],
                direction: OrderDirection::Asc,
                nulls: None,
            },
            OrderByItem {
                field: vec!["email".to_string()],
                direction: OrderDirection::Asc,
                nulls: None,
            },
        ],
    };

    {
        let projected = projected.clone();
        let order_by_id = order_by_id.clone();
        h.bench_batched(
            "order_by_single_column_typed/id_i64_asc_full",
            move || projected.clone(),
            move |mut recs| {
                apply_order_by_qv(&mut recs, &order_by_id);
                black_box(recs);
            },
        );
    }
    {
        let projected = projected.clone();
        let order_by_score = order_by_score.clone();
        h.bench_batched(
            "order_by_single_column_typed/score_f64_asc_full",
            move || projected.clone(),
            move |mut recs| {
                apply_order_by_qv(&mut recs, &order_by_score);
                black_box(recs);
            },
        );
    }
    {
        let projected = projected.clone();
        let order_by_email = order_by_email.clone();
        h.bench_batched(
            "order_by_single_column_typed/email_str_asc_full",
            move || projected.clone(),
            move |mut recs| {
                apply_order_by_qv(&mut recs, &order_by_email);
                black_box(recs);
            },
        );
    }
    {
        let projected = projected.clone();
        let order_by_active = order_by_active.clone();
        h.bench_batched(
            "order_by_single_column_typed/active_bool_asc_full",
            move || projected.clone(),
            move |mut recs| {
                apply_order_by_qv(&mut recs, &order_by_active);
                black_box(recs);
            },
        );
    }

    // ── Multi-column / fallback path (must not regress) ───────────
    {
        let projected = projected.clone();
        h.bench_batched(
            "order_by_multi_column/active_then_email_asc_full",
            move || projected.clone(),
            move |mut recs| {
                apply_order_by_qv(&mut recs, &order_by_multi);
                black_box(recs);
            },
        );
    }

    h.run();
}

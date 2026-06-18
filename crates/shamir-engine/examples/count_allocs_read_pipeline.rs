//! Query-scoped allocation baseline (#107).
//!
//! Runs the full read pipeline (SELECT * → ORDER BY → LIMIT 100) over
//! 100 k records under a **counting allocator** and prints total
//! allocations, bytes, and per-record averages.
//!
//! Run:
//!   cargo run --release --example count_allocs_read_pipeline
//!
//! Note: J1 migration — apply_select (JSON) removed; pipeline now uses
//! apply_select_value (QueryValue) + apply_order_by_qv. The allocation
//! counts will differ from the 2026-05-26 baseline below, which measured
//! the JSON path.
//!
//! Measured 2026-05-26 (release build, 100k records, full read pipeline, JSON path):
//!   - Total allocations:        1 600 007
//!   - Total bytes allocated:    140.5 MB
//!   - Allocs per record:         16.0
//!   - Bytes per record:          1 474
//!
//! Phase breakdown (JSON path):
//!   - apply_select:     800 002 allocs  (8.0/rec)   68.7 MB  61.6% of time
//!   - apply_order_by:   800 005 allocs  (8.0/rec)   71.8 MB  14.9% of time
//!   - apply_pagination:       0 allocs                0 MB   23.6% of time
//!
//! Verdict: >5% — PROCEED with #70 (arena allocator). The read pipeline
//! is allocation-bound. Every per-record json::Map + String key can be
//! served from a bump allocator that resets per query.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use shamir_engine::query::read::exec::{apply_order_by_qv, apply_pagination, apply_select_value};
use shamir_engine::query::read::{
    OrderBy, OrderByItem, OrderDirection, Pagination, Select, SelectItem,
};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

// ── Counting allocator ─────────────────────────────────────────────

static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOC_COUNT: AtomicU64 = AtomicU64::new(0);

struct Counter;

unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: Counter = Counter;

fn snapshot() -> (u64, u64, u64) {
    (
        ALLOC_COUNT.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
        DEALLOC_COUNT.load(Ordering::Relaxed),
    )
}

// ── Helpers ────────────────────────────────────────────────────────

fn touch(interner: &Interner, s: &str) -> InternerKey {
    match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    }
}

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
        touch(interner, "created_at"),
        InnerValue::Int(1_700_000_000 + idx as i64),
    );
    InnerValue::Map(m)
}

fn main() {
    let n: u64 = 100_000;

    // ── Phase 0: baseline (allocator overhead without data) ────────
    let (a0, b0, d0) = snapshot();

    // ── Phase 1: build records ─────────────────────────────────────
    let interner = Interner::new();
    for k in ["id", "name", "email", "score", "created_at"] {
        let _ = interner.touch_ind(k);
    }

    let t0 = Instant::now();
    let raw: Vec<(RecordId, InnerValue)> = (0..n)
        .map(|i| (RecordId::new(), make_record(&interner, i as u32)))
        .collect();
    let t_create = t0.elapsed();
    let (a1, b1, d1) = snapshot();

    // ── Phase 2: apply_select_value (SELECT *) ─────────────────────
    let select_all = Select {
        items: vec![SelectItem::All],
        distinct: false,
    };

    let t1 = Instant::now();
    let projected = apply_select_value(&raw, &select_all, &interner);
    let t_select = t1.elapsed();
    let (a2, b2, d2) = snapshot();

    // ── Phase 3: apply_order_by_qv ─────────────────────────────────
    let order_by_email = OrderBy {
        items: vec![OrderByItem {
            field: vec!["email".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };

    let mut sorted = projected.clone();
    let t2 = Instant::now();
    apply_order_by_qv(&mut sorted, &order_by_email);
    let t_order = t2.elapsed();
    let (a3, b3, d3) = snapshot();

    // ── Phase 4: apply_pagination (LIMIT 100) ─────────────────────
    let t3 = Instant::now();
    let (limited, _count) = apply_pagination(
        sorted,
        &Pagination::LimitOffset {
            limit: Some(100),
            offset: 0,
        },
        false,
    );
    let t_pag = t3.elapsed();
    let (a4, b4, d4) = snapshot();

    // ── Drop everything (measure deallocation pressure) ────────────
    drop(limited);
    drop(raw);
    drop(projected);
    let (a_drop, b_drop, d_drop) = snapshot();

    // ════════════════════════════════════════════════════════════════
    // REPORT
    // ════════════════════════════════════════════════════════════════
    let delta_alloc = |before: (u64, u64, u64), after: (u64, u64, u64)| -> (u64, u64, u64) {
        (after.0 - before.0, after.1 - before.1, after.2 - before.2)
    };

    let create = delta_alloc((a0, b0, d0), (a1, b1, d1));
    let select = delta_alloc((a1, b1, d1), (a2, b2, d2));
    let order = delta_alloc((a2, b2, d2), (a3, b3, d3));
    let pag = delta_alloc((a3, b3, d3), (a4, b4, d4));
    let drop_all = delta_alloc((a4, b4, d4), (a_drop, b_drop, d_drop));

    let total_pipeline_allocs = select.0 + order.0 + pag.0;
    let total_pipeline_bytes = select.1 + order.1 + pag.1;

    println!("\n╔════════════════════════════════════════════════════════════╗");
    println!("║  QUERY-SCOPED ALLOCATION BASELINE  (#107)                 ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();
    println!("Records: {n}  (QueryValue path — JSON path removed in J1)");
    println!();

    println!("── Phase timings ──────────────────────────────────────────");
    println!("  create records:      {t_create:>10?}");
    println!("  apply_select_value:  {t_select:>10?}");
    println!("  apply_order_by_qv:   {t_order:>10?}");
    println!("  apply_pagination:    {t_pag:>10?}");
    let total_time = t_select + t_order + t_pag;
    println!("  total pipeline:      {total_time:>10?}");
    println!();

    let fmt_phase = |name: &str, d: (u64, u64, u64)| {
        println!(
            "  {:24}  allocs={:>12}  bytes={:>12} ({:.1} MB)  deallocs={:>12}",
            name,
            d.0,
            d.1,
            d.1 as f64 / (1024.0 * 1024.0),
            d.2,
        );
    };

    println!("── Allocation counts per phase ────────────────────────────");
    fmt_phase("create_records", create);
    fmt_phase("apply_select_value", select);
    fmt_phase("apply_order_by_qv", order);
    fmt_phase("apply_pagination", pag);
    fmt_phase("drop_all", drop_all);
    println!();

    println!("── Pipeline totals (select + order + paginate) ────────────");
    println!("  Total allocations:     {}", total_pipeline_allocs);
    println!(
        "  Total bytes allocated: {} ({:.1} MB)",
        total_pipeline_bytes,
        total_pipeline_bytes as f64 / (1024.0 * 1024.0),
    );
    println!(
        "  Allocs per record:     {:.1}",
        total_pipeline_allocs as f64 / n as f64,
    );
    println!(
        "  Bytes per record:      {:.0}",
        total_pipeline_bytes as f64 / n as f64,
    );
    println!();

    println!("── Breakdown as % of pipeline ─────────────────────────────");
    let pct = |v: u64, total: u64| -> f64 {
        if total == 0 {
            0.0
        } else {
            v as f64 / total as f64 * 100.0
        }
    };
    println!(
        "  apply_select_value:  allocs {:.1}%  bytes {:.1}%",
        pct(select.0, total_pipeline_allocs),
        pct(select.1, total_pipeline_bytes),
    );
    println!(
        "  apply_order_by_qv:   allocs {:.1}%  bytes {:.1}%",
        pct(order.0, total_pipeline_allocs),
        pct(order.1, total_pipeline_bytes),
    );
    println!(
        "  apply_pagination:    allocs {:.1}%  bytes {:.1}%",
        pct(pag.0, total_pipeline_allocs),
        pct(pag.1, total_pipeline_bytes),
    );
    println!();

    println!("── Arena candidate sites ─────────────────────────────────");
    println!("  The following allocation sites are query-scoped (alloc during");
    println!("  query, free when result dropped) and would be absorbed by a");
    println!("  bump allocator (bumpalo) that resets per query:");
    println!();
    println!("  1. apply_select_value → inner_value_to_query_value(Map branch):");
    println!("       TMap::new() per record  (IndexMap alloc)");
    println!("       deintern_key → String::from(key)             per field per record");
    println!("  2. apply_order_by_qv → QvSortKey::Str clone per row");
    println!("  3. Vec<QueryValue> collect (result vec)           per batch");
    println!();
}

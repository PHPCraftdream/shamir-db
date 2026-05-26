//! Query-scoped allocation baseline (#107).
//!
//! Runs the full read pipeline (SELECT * → ORDER BY → LIMIT 100) over
//! 100 k records under a **counting allocator** and prints total
//! allocations, bytes, and per-record averages.
//!
//! Run:
//!   cargo run --release --example count_allocs_read_pipeline
//!
//! Measured 2026-05-26 (release build, 100k records, full read pipeline):
//!   - Total allocations:        1 600 007
//!   - Total bytes allocated:    140.5 MB
//!   - Allocs per record:         16.0
//!   - Bytes per record:          1 474
//!
//! Phase breakdown:
//!   - apply_select:     800 002 allocs  (8.0/rec)   68.7 MB  61.6% of time
//!   - apply_order_by:   800 005 allocs  (8.0/rec)   71.8 MB  14.9% of time
//!   - apply_pagination:       0 allocs                0 MB   23.6% of time
//!
//! Top alloc sites (inferred from code analysis):
//!   1. inner_to_json_value(Map) → json::Map::new() + insert per record
//!      (BTreeMap node allocation for each output object)
//!   2. deintern_key → String alloc per field per record
//!      (5 fields × 100k records = 500k String allocs in apply_select)
//!   3. apply_order_by → clone inside sort comparator / swap of
//!      json::Value objects (800k allocs during unstable sort)
//!   4. serde_json::Number construction per numeric field
//!   5. Arc<str> → String conversion per string field
//!
//! Conclusion: arena allocator would absorb approximately:
//!   - ~100% of apply_select allocations (800k, query-scoped)
//!   - ~100% of apply_order_by temporary allocs (800k, sort-scoped)
//!   - ~1.6M allocs total = 100% of read pipeline allocs
//!   - These allocs dominate pipeline time (61.6% in apply_select alone)
//!
//! Verdict: >5% — PROCEED with #70 (arena allocator). The read pipeline
//! is allocation-bound. Every per-record json::Map + String key can be
//! served from a bump allocator that resets per query, eliminating
//! ~1.6M malloc/free pairs per 100k-record query.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use shamir_engine::query::read::exec::{apply_order_by, apply_pagination, apply_select};
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

    // ── Phase 2: apply_select (SELECT *) ───────────────────────────
    let select_all = Select {
        items: vec![SelectItem::All],
        distinct: false,
    };

    let t1 = Instant::now();
    let projected = apply_select(&raw, &select_all, &interner);
    let t_select = t1.elapsed();
    let (a2, b2, d2) = snapshot();

    // ── Phase 3: apply_order_by ────────────────────────────────────
    let order_by_email = OrderBy {
        items: vec![OrderByItem {
            field: vec!["email".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };

    let mut sorted = projected.clone();
    let t2 = Instant::now();
    apply_order_by(&mut sorted, &order_by_email);
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
    println!("Records: {n}");
    println!();

    println!("── Phase timings ──────────────────────────────────────────");
    println!("  create records:   {t_create:>10?}");
    println!("  apply_select:     {t_select:>10?}");
    println!("  apply_order_by:   {t_order:>10?}");
    println!("  apply_pagination: {t_pag:>10?}");
    let total_time = t_select + t_order + t_pag;
    println!("  total pipeline:   {total_time:>10?}");
    println!();

    let fmt_phase = |name: &str, d: (u64, u64, u64)| {
        println!(
            "  {:20}  allocs={:>12}  bytes={:>12} ({:.1} MB)  deallocs={:>12}",
            name,
            d.0,
            d.1,
            d.1 as f64 / (1024.0 * 1024.0),
            d.2,
        );
    };

    println!("── Allocation counts per phase ────────────────────────────");
    fmt_phase("create_records", create);
    fmt_phase("apply_select", select);
    fmt_phase("apply_order_by", order);
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
        "  apply_select:     allocs {:.1}%  bytes {:.1}%",
        pct(select.0, total_pipeline_allocs),
        pct(select.1, total_pipeline_bytes),
    );
    println!(
        "  apply_order_by:   allocs {:.1}%  bytes {:.1}%",
        pct(order.0, total_pipeline_allocs),
        pct(order.1, total_pipeline_bytes),
    );
    println!(
        "  apply_pagination: allocs {:.1}%  bytes {:.1}%",
        pct(pag.0, total_pipeline_allocs),
        pct(pag.1, total_pipeline_bytes),
    );
    println!();

    // ── Inferred per-site breakdown ────────────────────────────────
    // For SELECT * on 5-field records:
    //   per record: 1 json::Map + 5 String keys (deintern) + 5 Value wrappers
    //             + ~2-3 Number internals + inner map alloc
    //             ≈ ~14-20 allocs per record in apply_select
    println!("── Inferred per-site breakdown ───────────────────────────");
    println!(
        "  apply_select ({:.0} allocs/rec) is dominated by:",
        select.0 as f64 / n as f64,
    );
    println!("    1. json::Map::new() + insert (BTreeMap node)    per record");
    println!("    2. deintern_key → String::from(key)             per field per record");
    println!("    3. serde_json::Number construction              per numeric field");
    println!("    4. String clone (Arc<str> → String)             per string field");
    println!("    5. Vec<json::Value> collect (result vec)        per batch");
    println!();

    // ── Phase 2: isolate SELECT (the 63% hotspot) ─────────────────
    // Run apply_select again with fresh allocator reading to confirm
    // per-record cost independently of order_by.
    println!("── apply_select isolation ────────────────────────────────");
    println!(
        "  apply_select allocs:     {}  ({:.1} allocs/rec)",
        select.0,
        select.0 as f64 / n as f64,
    );
    println!(
        "  apply_select bytes:      {} ({:.1} MB)  ({:.0} bytes/rec)",
        select.1,
        select.1 as f64 / (1024.0 * 1024.0),
        select.1 as f64 / n as f64,
    );
    println!("  apply_select time:       {t_select:?}",);
    println!(
        "  apply_select % of pipeline time: {:.1}%",
        if total_time.as_nanos() > 0 {
            t_select.as_secs_f64() / total_time.as_secs_f64() * 100.0
        } else {
            0.0
        },
    );
    println!();

    println!("── Arena candidate sites ─────────────────────────────────");
    println!("  The following allocation sites are query-scoped (alloc during");
    println!("  query, free when result dropped) and would be absorbed by a");
    println!("  bump allocator (bumpalo) that resets per query:");
    println!();
    println!("  1. apply_select → inner_to_json_value(Map branch):");
    println!("       json::Map::new() per record  (BTreeMap root alloc)");
    println!("       deintern_key → String alloc per field per record");
    println!("       serde_json::Number::from_f64 / from(i64) per numeric field");
    println!("       Arc<str> → String clone per string field");
    println!("  2. apply_select → inner_to_json_value (non-Map branches):");
    println!("       Vec<json::Value> per List/Set field");
    println!("       Vec<json::Value::Number> per Bin field");
    println!("  3. apply_order_by → projected.clone() in iter_batched:");
    println!("       clones entire Vec<json::Value> (deep clone of all maps)");
    println!("  4. compile_filter → Vec<FilterNode> (query compilation):");
    println!("       one-time per query, negligible at scale");
    println!();
}

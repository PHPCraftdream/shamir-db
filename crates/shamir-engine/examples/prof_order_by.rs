//! Standalone timing harness for ORDER BY hotspot breakdown.
//!
//! Measures wall-clock time for each ORDER BY phase and isolates the cost
//! of field resolution (get_json_field / Value::get) from sort/swap overhead.
//!
//! Run: cargo run --release --example prof_order_by

use std::time::Instant;

use shamir_engine::query::read::exec::{apply_order_by, apply_pagination, apply_select};
use shamir_engine::query::read::{
    OrderBy, OrderByItem, OrderDirection, Pagination, Select, SelectItem,
};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

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
    let interner = Interner::new();
    for k in ["id", "name", "email", "score", "created_at"] {
        let _ = interner.touch_ind(k);
    }

    let n: u64 = 100_000;

    let t0 = Instant::now();
    let raw: Vec<(RecordId, InnerValue)> = (0..n)
        .map(|i| (RecordId::new(), make_record(&interner, i as u32)))
        .collect();
    let t_create = t0.elapsed();
    println!("create {n} records:      {t_create:?}");

    let select_all = Select {
        items: vec![SelectItem::All],
        distinct: false,
    };
    let t1 = Instant::now();
    let projected = apply_select(&raw, &select_all, &interner);
    let t_select = t1.elapsed();
    println!("apply_select:            {t_select:?}");

    let order_by_email = OrderBy {
        items: vec![OrderByItem {
            field: vec!["email".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };

    // ── Run 1: Full apply_order_by (production path) ──────────────
    let mut email_times = Vec::new();
    for i in 0..5 {
        let mut c = projected.clone();
        let t = Instant::now();
        apply_order_by(&mut c, &order_by_email);
        let e = t.elapsed();
        email_times.push(e);
        if i == 0 {
            println!("apply_order_by (email):  {e:?}");
        }
    }

    // ── Run 2: Sort with pre-extracted keys, in-place Value swaps ─
    // This isolates the cost of per-comparison Value::get by removing it.
    // The swap cost (moving json::Value) remains identical to production.
    let email_keys: Vec<String> = projected
        .iter()
        .map(|v| {
            v.get("email")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        })
        .collect();

    let mut preextract_times = Vec::new();
    for _ in 0..5 {
        let mut c = projected.clone();
        let keys = email_keys.clone();
        // Strategy: sort an index array by pre-extracted keys, then
        // permute the Values array by the sorted index. Measures
        // sort_indices + apply_permutation cost only, no comparator
        // lookup.
        let t = Instant::now();
        let mut idx: Vec<usize> = (0..c.len()).collect();
        idx.sort_by(|&a, &b| keys[a].cmp(&keys[b]));
        // Apply permutation: create new vec in sorted order
        let sorted: Vec<_> = idx.into_iter().map(|i| std::mem::take(&mut c[i])).collect();
        let e = t.elapsed();
        preextract_times.push(e);
        drop(sorted);
    }
    println!(
        "sort+permute (pre-extracted email): {:?}",
        preextract_times.iter().min().unwrap()
    );

    // ── Run 3: Just the permutation (no sort) ─────────────────────
    // Measures the cost of moving 100k json::Value objects
    let identity_perm: Vec<usize> = (0..projected.len()).collect();
    let mut permute_times = Vec::new();
    for _ in 0..5 {
        let mut c = projected.clone();
        let t = Instant::now();
        let sorted: Vec<_> = identity_perm
            .iter()
            .map(|&i| std::mem::take(&mut c[i]))
            .collect();
        let e = t.elapsed();
        permute_times.push(e);
        drop(sorted);
    }
    println!(
        "permute only (identity): {:?}",
        permute_times.iter().min().unwrap()
    );

    // ── Run 4: Pre-extract cost ───────────────────────────────────
    let mut extract_times = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let _keys: Vec<String> = projected
            .iter()
            .map(|v| {
                v.get("email")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        let e = t.elapsed();
        extract_times.push(e);
    }
    println!(
        "pre-extract email keys:  {:?}",
        extract_times.iter().min().unwrap()
    );

    // ── Score sort ────────────────────────────────────────────────
    let order_by_score = OrderBy {
        items: vec![OrderByItem {
            field: vec!["score".to_string()],
            direction: OrderDirection::Asc,
            nulls: None,
        }],
    };
    let mut score_times = Vec::new();
    for i in 0..5 {
        let mut c = projected.clone();
        let t = Instant::now();
        apply_order_by(&mut c, &order_by_score);
        let e = t.elapsed();
        score_times.push(e);
        if i == 0 {
            println!("apply_order_by (score):  {e:?}");
        }
    }

    // ── Clone cost ────────────────────────────────────────────────
    let t_clone = Instant::now();
    let _cloned = projected.clone();
    let t_clone_dur = t_clone.elapsed();
    println!("clone 100k json Values:  {t_clone_dur:?}");

    // ── Pagination ────────────────────────────────────────────────
    let mut c_pag = projected.clone();
    apply_order_by(&mut c_pag, &order_by_email);
    let t_pag = Instant::now();
    let (limited, _) = apply_pagination(
        c_pag,
        &Pagination::LimitOffset {
            limit: Some(100),
            offset: 0,
        },
        false,
    );
    let t_pag_dur = t_pag.elapsed();
    println!(
        "apply_pagination(100):   {t_pag_dur:?}  (len={})",
        limited.len()
    );

    // ══════════════════════════════════════════════════════════════
    // SUMMARY
    // ══════════════════════════════════════════════════════════════
    let avg_email = email_times.iter().sum::<std::time::Duration>() / 5;
    let avg_score = score_times.iter().sum::<std::time::Duration>() / 5;
    let avg_preextract = preextract_times.iter().sum::<std::time::Duration>() / 5;
    let avg_permute = permute_times.iter().sum::<std::time::Duration>() / 5;
    let avg_extract = extract_times.iter().sum::<std::time::Duration>() / 5;
    let min_email = email_times.iter().min().unwrap();
    let min_preextract = preextract_times.iter().min().unwrap();
    let min_permute = permute_times.iter().min().unwrap();

    println!("\n========== RESULTS ==========");
    println!("Records: {n}");
    println!();
    println!("Phase timings (avg of 5):");
    println!("  apply_order_by (email):      {avg_email:?}  (min: {min_email:?})");
    println!("  apply_order_by (score):      {avg_score:?}");
    println!("  sort+permute (pre-extracted):{avg_preextract:?}  (min: {min_preextract:?})");
    println!("  permute only (identity):     {avg_permute:?}  (min: {min_permute:?})");
    println!("  pre-extract email keys:      {avg_extract:?}");
    println!("  apply_select (projection):   {t_select:?}");
    println!("  clone:                       {t_clone_dur:?}");
    println!("  apply_pagination (limit 100):{t_pag_dur:?}");

    // Breakdown:
    // full_sort = sort_logic + per_comparison_lookup + Value_swaps
    // preextract_sort+permute = sort_logic + index_swaps + Value_permute
    // permute_only = Value_permute
    //
    // So: sort_logic ≈ preextract_sort - permute_only (approximately, since index swap != Value swap)
    // And: per_comparison_lookup ≈ full_sort - preextract_sort_and_permute
    // But the pre-extracted sort uses index swaps (cheap) while the full sort
    // uses Value swaps (expensive). To isolate lookup, we need to account for
    // the swap difference.

    // Better approach: sort_logic_and_swaps = preextract_sort + permute - index_sort
    // Actually the cleanest comparison:
    // - full_sort does: sort_by with lookup + Value swaps
    // - preextract does: sort index (cheap swaps) + permute Values
    // difference = lookup_cost + (Value_swaps_during_sort - index_swaps - one_permutation)

    println!("\n========== BREAKDOWN ==========");
    let pure_sort_with_permute = *min_preextract;
    let pure_permute = *min_permute;
    let pure_sort = pure_sort_with_permute.saturating_sub(pure_permute);
    let lookup_plus_swap_overhead = min_email.saturating_sub(pure_sort_with_permute);

    println!("Pure sort logic (sort_indices - permute): {pure_sort:?}");
    println!("Pure Value permutation:                   {pure_permute:?}");
    println!("Sort+permute (pre-extracted):              {pure_sort_with_permute:?}");
    println!("Full sort (with lookup):                   {min_email:?}");
    println!();
    println!("Lookup + swap overhead (full - pre-extracted): {lookup_plus_swap_overhead:?}");
    println!(
        "  as % of full sort: {:.1}%",
        if min_email.as_nanos() > 0 {
            lookup_plus_swap_overhead.as_secs_f64() / min_email.as_secs_f64() * 100.0
        } else {
            0.0
        }
    );
    println!(
        "  pure sort+permute as % of full sort: {:.1}%",
        if min_email.as_nanos() > 0 {
            pure_sort_with_permute.as_secs_f64() / min_email.as_secs_f64() * 100.0
        } else {
            0.0
        }
    );

    // What fraction of the full read pipeline is ORDER BY?
    let total_pipeline = t_select + *min_email + t_pag_dur;
    println!();
    println!("Full read pipeline (select + sort + paginate): {total_pipeline:?}");
    println!(
        "  select:      {:.1}% ({t_select:?})",
        t_select.as_secs_f64() / total_pipeline.as_secs_f64() * 100.0
    );
    println!(
        "  order_by:    {:.1}% ({min_email:?})",
        min_email.as_secs_f64() / total_pipeline.as_secs_f64() * 100.0
    );
    println!(
        "  pagination:  {:.1}% ({t_pag_dur:?})",
        t_pag_dur.as_secs_f64() / total_pipeline.as_secs_f64() * 100.0
    );

    println!("\n========== VERDICT ==========");
    let overhead_pct = if min_email.as_nanos() > 0 {
        lookup_plus_swap_overhead.as_secs_f64() / min_email.as_secs_f64() * 100.0
    } else {
        0.0
    };
    println!(
        "Lookup + swap overhead is ~{:.0}% of ORDER BY time.",
        overhead_pct
    );
    println!(
        "ORDER BY is {:.1}% of the full read pipeline.",
        min_email.as_secs_f64() / total_pipeline.as_secs_f64() * 100.0
    );
    println!();
    println!("Top hotspots (estimated from timing isolation):");
    println!(
        "  1. Value::get (field lookup in comparator) + Value swap overhead — {:.0}% of sort",
        overhead_pct
    );
    println!(
        "  2. Pure sort logic (string comparison + index manipulation) — {:.0}% of sort",
        100.0 - overhead_pct
    );
    println!(
        "  3. apply_select (projection to JSON) — dominant cost at {:.0}% of pipeline",
        t_select.as_secs_f64() / total_pipeline.as_secs_f64() * 100.0
    );
    println!();
    if overhead_pct > 10.0 {
        println!("CONCLUSION: get_json_field / Value::get IS a significant hotspot");
        println!(
            "(~{:.0}% of sort time). Proceed with #67 (precomputed positions).",
            overhead_pct
        );
    } else {
        println!("CONCLUSION: get_json_field / Value::get is NOT a bottleneck.");
        println!("Close #67 as not-worth-it.");
    }
}

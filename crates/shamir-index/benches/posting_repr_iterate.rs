//! Posting-list representation bench — audit `2026-07-06-perf-radical-o-notation`
//! finding §3.2 / task #499.
//!
//! The audit flagged that `IndexManager::lookup_by_index` returned a
//! `BTreeSet<RecordId>` even though the underlying prefix scan already yields
//! sorted, unique ids, and that EVERY consumer only ITERATES the result (to
//! union into a downstream set, count, or check emptiness). Task #499 replaced
//! the representation with a sorted `Arc<[RecordId]>`.
//!
//! # What this bench measures — a REAL before/after
//!
//! Unlike `posting_cache_hit` (which measures the O(1) cache `Arc::clone` and
//! so is flat across representations), this bench isolates the CONSUMER cost:
//! given `n` postings, iterate the whole posting set and union-collect the ids
//! into a fresh result set — exactly what `read_index_scan` /
//! `write_helpers` do (`record_ids.extend(ids.iter().copied())`).
//!
//! Both representations are materialised from the SAME sorted id vector in one
//! bench binary, so the two columns are a genuine, reproducible before/after:
//!
//! - `iterate_union/btreeset/<n>` — old path: iterate a `BTreeSet<RecordId>`
//!   (cache-unfriendly pointer chasing across tree nodes) and collect.
//! - `iterate_union/sorted_slice/<n>` — new path: iterate a contiguous
//!   `[RecordId]` slice and collect.
//!
//! The audit's stated bench gap was equality-lookup coverage at
//! |postings| >= 10k; this covers 10k and 100k.
//!
//! Run: `cargo bench -p shamir-index --bench posting_repr_iterate`
//! (use the isolated bench target dir per the workspace convention).

use std::collections::BTreeSet;
use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_types::types::record_id::RecordId;

/// Builds `n` distinct, sorted `RecordId`s sharing one microsecond timestamp
/// with ascending seq tails — the low-cardinality bucket the audit cites
/// (`status = 'active'` with many postings). Returned already sorted.
fn sorted_ids(n: usize) -> Vec<RecordId> {
    let ts = RecordId::now_micros();
    (0..n as u32)
        .map(|i| RecordId::from_ts_seq(ts, i))
        .collect()
}

fn main() {
    let mut h = Harness::new("posting_repr_iterate", env!("CARGO_MANIFEST_DIR"));

    for &n in &[10_000usize, 100_000usize] {
        let ids = sorted_ids(n);

        // OLD representation: a BTreeSet built from the sorted ids.
        let set: BTreeSet<RecordId> = ids.iter().copied().collect();
        // NEW representation: the sorted slice (as an owned Vec here; in prod
        // it is `Arc<[RecordId]>`, but iteration cost is identical).
        let slice: Vec<RecordId> = ids.clone();

        // Old path: iterate the tree + union-collect.
        {
            let set = set.clone();
            let id = format!("iterate_union/btreeset/{}k", n / 1_000);
            h.bench(&id, move || {
                let mut out: BTreeSet<RecordId> = BTreeSet::new();
                out.extend(set.iter().copied());
                black_box(out.len());
            });
        }

        // New path: iterate the contiguous slice + union-collect.
        {
            let slice = slice.clone();
            let id = format!("iterate_union/sorted_slice/{}k", n / 1_000);
            h.bench(&id, move || {
                let mut out: BTreeSet<RecordId> = BTreeSet::new();
                out.extend(slice.iter().copied());
                black_box(out.len());
            });
        }
    }

    h.run();
}

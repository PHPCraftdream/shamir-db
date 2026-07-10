//! Task #500 soak — WAL startup (`SegmentSet::open`) time vs. accumulated
//! sealed segments.
//!
//! The audit (finding 2.1) noted NO bench existed for WAL startup/recovery
//! time and called for a soak: "накопить N сегментов → замерить open". This is
//! that soak. It accumulates N sealed segments of realistic size, then times
//! `SegmentSet::open` in two on-disk states:
//!
//!   - `with_sidecar`  — segments carry the `NNNNNNNN.meta` max_version sidecar
//!     this change writes at seal time. `open` reads a ~17-byte sidecar per
//!     sealed segment and SKIPS the full replay. Cost ≈ O(segment_count).
//!   - `full_replay`   — the pre-#500 behaviour, reproduced by deleting every
//!     sidecar so `open` falls back to a full `replay_sealed_at_startup()` of
//!     every sealed segment. Cost ≈ O(total on-disk WAL bytes).
//!
//! The ratio between the two IDs at a given N is the startup speedup. Both
//! directories are built ONCE (untimed) and `open`ed repeatedly; `open` is
//! read-only over the on-disk segments (it never mutates sealed files), so
//! re-opening the same directory each iteration is idempotent and measures the
//! steady-state open path.

use bench_scale_tool::Harness;
use shamir_types::types::record_id::RecordId;
use shamir_wal::segment_set::SegmentSet;
use shamir_wal::wal_entry_v2::{WalEntryV2, WalOpV2};

/// Sealed-segment counts to soak over. Each is a distinct directory.
const SEGMENT_COUNTS: &[usize] = &[16, 64, 256];

/// Records packed into each sealed segment before it rotates. Drives the
/// per-segment replay cost (the whole point of the fast path is to avoid
/// decoding these). ~200 records × ~a few hundred bytes ≈ a realistic segment.
const RECORDS_PER_SEGMENT: usize = 200;

/// Filler body size per record so each frame is a realistic width (an encoded
/// MVCC put with a small field set), making replay decode non-trivial.
const BODY_LEN: usize = 256;

fn rid(n: u64) -> RecordId {
    let mut a = [0u8; 16];
    a[8..16].copy_from_slice(&n.to_le_bytes());
    RecordId(a)
}

fn record(version: u64) -> Vec<u8> {
    WalEntryV2::new(
        version,
        7,
        vec![WalOpV2::Put {
            table_id_interned: 7,
            rid: rid(version),
            body: vec![0xABu8; BODY_LEN].into(),
        }],
    )
    .with_commit_version(version)
    .encode()
    .expect("encode")
}

/// Build a directory holding `segments` sealed segments (+ one active). Uses a
/// seal threshold sized so exactly `RECORDS_PER_SEGMENT` records fill a segment
/// before it rotates. Returns the populated `TempDir` (kept alive by caller).
fn build_dir(segments: usize) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        // Threshold: one record's frame is `len(4) + payload + crc(4)`. Set the
        // seal threshold to RECORDS_PER_SEGMENT frames so a segment rotates
        // after that many appends.
        let frame = record(1).len() as u64 + 8;
        let max_bytes = frame * RECORDS_PER_SEGMENT as u64;
        let set = SegmentSet::open(dir.path().to_path_buf(), max_bytes)
            .await
            .expect("open");
        // Enough records to seal `segments` segments (plus a partial active).
        let total = (segments + 1) * RECORDS_PER_SEGMENT;
        for v in 1..=total as u64 {
            set.append_batch(vec![record(v)], v).await.expect("append");
        }
        set.sync().await.expect("sync");
    });
    dir
}

/// Delete every `.meta` sidecar in `dir`, forcing `open` down the full-replay
/// fallback (reproduces pre-#500 startup behaviour).
fn strip_sidecars(dir: &std::path::Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|e| e.to_str()) == Some("meta") {
            std::fs::remove_file(&p).unwrap();
        }
    }
}

/// Time one `SegmentSet::open` over `dir` (the whole open, including the
/// sealed-segment max_version resolution — sidecar fast path or replay).
fn open_once(dir: &std::path::Path) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let set = SegmentSet::open(dir.to_path_buf(), 1 << 30)
            .await
            .expect("open");
        // Touch max_committed so the compiler cannot elide the sealed-meta work.
        std::hint::black_box(set.max_committed());
    });
}

fn main() {
    let mut h = Harness::new("wal_startup_open", env!("CARGO_MANIFEST_DIR"));

    for &n in SEGMENT_COUNTS {
        // ── fast path: sidecars present ────────────────────────────────────
        let dir_fast = build_dir(n);
        let id = format!("wal_startup_open/with_sidecar/segs_{n}");
        h.bench(&id, move || {
            let _keep = &dir_fast;
            open_once(dir_fast.path());
        });

        // ── fallback: no sidecars → full replay of every sealed segment ────
        let dir_slow = build_dir(n);
        strip_sidecars(dir_slow.path());
        let id = format!("wal_startup_open/full_replay/segs_{n}");
        h.bench(&id, move || {
            let _keep = &dir_slow;
            open_once(dir_slow.path());
        });
    }

    h.run();
}

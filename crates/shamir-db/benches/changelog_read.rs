//! Journal read / changelog backfill micro-benchmark.
//!
//! Measures `ShamirDb::read_changelog_from(db, repo, from_version, limit)` —
//! the resumable pull path that backs `SubscribeOp { from_version }`
//! catchup today and will back the leader→follower replication pull loop
//! tomorrow.
//!
//! This is the pre-work for the upcoming replication track: capture
//! baseline latency BEFORE any replication code is built on top.
//!
//! One axis, one cell:
//!   * **backfill depth** — 100 (recent) — the smallest scale. `limit` = 100
//!     (one full page).
//!
//! The Criterion-era sweep over depth ∈ {100, 1_000, 10_000} × limit ∈
//! {100, 1_000} has been collapsed to the single smallest cell: the
//! fixed-iteration harness (`bench_scale_tool`) now owns repetition count,
//! so each registered call must be a cheap ≲10ms unit. The deeper cells
//! cost 5-8ms per single call at the old sizes, which left no headroom
//! once the harness multiplied them — the depth_100/limit_100 cell (~0.4ms)
//! is the unit that stays.
//!
//! Setup populates the journal with ~200 single-row commits (one commit per
//! `db.execute` call) so the depth_100 backfill has real data behind it.
//! Setup runs ONCE, shared across every iteration — plan 1 (`bench_async`),
//! since `read_changelog_from` never mutates the journal.
//!
//! Run:
//!   cargo bench -p shamir-db --bench changelog_read

use std::hint::black_box;
use std::sync::Arc;

include!("bench_allocator.rs");

use bench_scale_tool::Harness;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::write;

const DB: &str = "bench";
const REPO: &str = "main";
const TABLE: &str = "users";

/// This bench's depth × limit axis exists to show how backfill-read
/// latency scales as `from_version` sits further behind the current
/// commit (up to `JOURNAL_BACKFILL_LIMIT`) — a genuine structural
/// comparison, not an artificial per-op loop the harness's own repetition
/// count already covers. Default sweep keeps depth=100/limit=100 (~0.4ms/
/// call); the deeper cells (depth up to 10_000, near
/// `shamir_tunables::instance_defaults::JOURNAL_BACKFILL_LIMIT`, cost
/// 5-8ms/call each) are opt-in via BENCH_CHANGELOG_SCALING=1 so that
/// signal isn't lost, just not in the default fast path.
fn depth_limit_cells() -> Vec<(u64, usize)> {
    let wide = std::env::var("BENCH_CHANGELOG_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    if wide {
        let mut cells = Vec::new();
        for &depth in &[100u64, 1_000, 10_000] {
            for &limit in &[100usize, 1_000] {
                cells.push((depth, limit));
            }
        }
        cells
    } else {
        vec![(100u64, 100usize)]
    }
}

/// Number of journal events to seed before benchmarking. Must exceed the
/// deepest registered `depth` so `from_version = current - depth` is
/// valid and the journal genuinely has to scan/return real events.
fn seed_commits() -> usize {
    let wide = std::env::var("BENCH_CHANGELOG_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    if wide {
        12_000
    } else {
        200
    }
}

/// One-row, transactional insert batch. Each `execute` call yields one
/// `commit_version` → one journal entry.
fn one_row_insert(i: usize) -> shamir_db::query::batch::BatchRequest {
    let mut b = Batch::new();
    b.id("ins").transactional();
    b.insert(
        "i",
        write::insert(TABLE).row(mpack!({
            "id":   @(QueryValue::from(format!("u{:08}", i))),
            "name": "x",
            "age":  @(QueryValue::from((i % 90) as i64)),
        })),
    );
    b.build()
}

/// Fresh in-memory ShamirDb with one repo + one table, populated with
/// `seed_commits()` independent commits so the journal has ≥ depth events.
async fn seeded_journal() -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db(DB).await;
    let cfg = RepoConfig::new(REPO, BoxRepoFactory::in_memory()).add_table(TableConfig::new(TABLE));
    shamir.add_repo(DB, cfg).await.expect("add_repo");

    for i in 0..seed_commits() {
        let req = one_row_insert(i);
        shamir.execute(DB, &req).await.expect("seed insert");
    }
    shamir
}

fn main() {
    let mut h = Harness::new("changelog_read", env!("CARGO_MANIFEST_DIR"));

    let (shamir, current) = {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let shamir = rt.block_on(seeded_journal());
        // Anchor at the current commit_version so every iteration reads the
        // same window — no drift across samples.
        let current = rt
            .block_on(shamir.current_commit_version(DB, REPO))
            .expect("repo has a commit version");
        (shamir, current)
    };

    for (depth, limit) in depth_limit_cells() {
        // from_version = current - depth + 1 → exactly `depth` events sit at
        // or after from_version. With `limit ≤ depth` the read returns a full
        // page.
        let from_version = current.saturating_sub(depth) + 1;
        let shamir = Arc::clone(&shamir);
        let id = format!("changelog_read_from/from_depth_{depth}_lim_{limit}");
        h.bench_async(&id, move || {
            let s = Arc::clone(&shamir);
            async move {
                let events = s
                    .read_changelog_from(DB, REPO, from_version, limit)
                    .await
                    .expect("repo exists");
                black_box(events);
            }
        });
    }

    h.run();
}

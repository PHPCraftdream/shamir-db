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
//! Two axes:
//!   * **backfill depth** — how far `from_version` sits behind the
//!     current commit_version: 100 (recent), 1_000, 10_000 (close to
//!     `shamir_tunables::instance_defaults::JOURNAL_BACKFILL_LIMIT`).
//!   * **limit** — page size: 100, 1_000.
//!
//! Setup populates the journal with ~12_000 single-row commits (one
//! commit per `db.execute` call) so even the deepest backfill has real
//! data behind it. Migrated to the fixed-iteration harness
//! (`bench_scale_tool`): setup runs ONCE, shared across every iteration —
//! plan 1 (`bench_async`), since `read_changelog_from` never mutates the
//! journal.
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
/// Number of journal events to seed before benchmarking. Must exceed the
/// deepest `depth` so `from_version = current - depth` is valid and
/// the journal genuinely has to scan/return real events.
const SEED_COMMITS: usize = 12_000;

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
/// `SEED_COMMITS` independent commits so the journal has ≥ depth events.
async fn seeded_journal() -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db(DB).await;
    let cfg = RepoConfig::new(REPO, BoxRepoFactory::in_memory()).add_table(TableConfig::new(TABLE));
    shamir.add_repo(DB, cfg).await.expect("add_repo");

    for i in 0..SEED_COMMITS {
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

    for &depth in &[100u64, 1_000, 10_000] {
        for &limit in &[100usize, 1_000] {
            // from_version = current - depth + 1 → exactly `depth` events
            // sit at or after from_version. With `limit ≤ depth` the read
            // returns a full page; with `limit > depth` it returns `depth`.
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
    }

    h.run();
}

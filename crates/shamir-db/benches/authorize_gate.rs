//! Benchmark for the Shomer authorization gate (P4).
//!
//! Measures two paths:
//!   1. System fast-path (admin bypass) — the common live path.
//!   2. Non-System path with meta resolution + ancestor traversal
//!      (a Table 3 levels deep under a Database).
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): DB setup
//! (create_db/add_repo, once) is done ONCE outside every timed closure —
//! plan 1 (shared setup) for all three workloads, since `authorize_access`
//! only reads the meta tree and never mutates it.
//!
//! Run:
//!   cargo bench -p shamir-db --bench authorize_gate

include!("bench_allocator.rs");

use bench_scale_tool::Harness;
use shamir_db::access::{Action, Actor, ResourcePath};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::shamir_db::ShamirDb;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    // `create_db`/`add_repo` (System-owned) now persist ResourceMeta::owned_enforced
    // (owner-only 0o700, "Strategy A") rather than the old open 0o777 default, so a
    // non-System actor is denied on every ancestor by default. This bench exists to
    // measure the *authorized* non-System traversal path (see module docs), so stamp
    // User(42) as owner via the `_as` variants — that makes it PermClass::Owner with
    // full rwx on db/repo/table, matching what `user_traverse_*` intends to exercise.
    let bench_user = Actor::User(42);
    shamir.create_db_as("benchdb", bench_user.clone()).await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("records"));
    shamir
        .add_repo_as("benchdb", config, bench_user)
        .await
        .unwrap();
    shamir
}

fn main() {
    let mut h = Harness::new("authorize_gate", env!("CARGO_MANIFEST_DIR"));

    // Setup runs ONCE at registration time, shared across every iteration
    // of every workload below (authorize_access is read-only).
    let shamir = {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(setup_shamir())
    };

    // 1. System fast-path (admin bypass).
    {
        let shamir = shamir.clone();
        h.bench_async("authorize_gate/system_bypass", move || {
            let shamir = shamir.clone();
            async move {
                shamir
                    .authorize_access(
                        &Actor::System,
                        &ResourcePath::table("benchdb", "data", "records"),
                        Action::Read,
                    )
                    .await
                    .unwrap();
            }
        });
    }

    // 2. Non-System path with traversal (3 ancestors + target).
    {
        let shamir = shamir.clone();
        h.bench_async("authorize_gate/user_traverse_table", move || {
            let shamir = shamir.clone();
            async move {
                shamir
                    .authorize_access(
                        &Actor::User(42),
                        &ResourcePath::table("benchdb", "data", "records"),
                        Action::Read,
                    )
                    .await
                    .unwrap();
            }
        });
    }

    // 3. Non-System path on a deep resource (Record — 4 ancestors).
    {
        let shamir = shamir.clone();
        h.bench_async("authorize_gate/user_traverse_record", move || {
            let shamir = shamir.clone();
            async move {
                shamir
                    .authorize_access(
                        &Actor::User(42),
                        &ResourcePath::record("benchdb", "data", "records", "key1"),
                        Action::Read,
                    )
                    .await
                    .unwrap();
            }
        });
    }

    h.run();
}

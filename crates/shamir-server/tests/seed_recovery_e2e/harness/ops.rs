//! Admin control-plane drivers (builder-only): profile + subscription
//! creation, validated Resume, state snapshots for no-mutation assertions,
//! and fixed-Hello resume probes for the mismatch scenarios.

use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::{ResumeProbe, SeedRefusal, ShamirDb};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::wire::repl::ReplResponse;

use super::follower;
use super::wire;

/// Create `profile` (Pull/ReadOnly over app/main) and `sub` pointed at
/// `upstream`. `paused` mirrors `ddl::paused_subscription`.
pub async fn create_subscription(home: &ShamirDb, upstream: &str, paused: bool) {
    let scope = ddl::repl_scope(wire::DB).repo(wire::REPO).build();
    let mut batch = Batch::new();
    batch.id("admin");
    batch.op(
        "profile",
        ddl::replication_profile("seedprof").stream(scope, ReplDirection::Pull, ReplMode::ReadOnly),
    );
    batch.op(
        "sub",
        if paused {
            ddl::paused_subscription("sub", upstream, wire::PUBLICATION, "seedprof")
        } else {
            ddl::subscription("sub", upstream, wire::PUBLICATION, "seedprof")
        },
    );
    let response = home
        .execute(wire::DB, &batch.build())
        .await
        .expect("admin ops");
    assert!(
        !response.results.contains_key("__error"),
        "subscription creation failed: {response:?}",
    );
}

/// The operator's explicit, validated Resume of `sub`.
pub async fn resume(home: &ShamirDb) -> Result<(), String> {
    let mut batch = Batch::new();
    batch.id("resume");
    batch
        .try_op("resume", ddl::alter_subscription("sub").resume())
        .expect("resume op builds");
    match home.execute(wire::DB, &batch.build()).await {
        Ok(response) => {
            assert!(
                !response.results.contains_key("__error"),
                "resume batch error: {response:?}",
            );
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Rows + bookmark only — the DATA a terminal stop must not mutate.
pub async fn data_fingerprint(home: &ShamirDb) -> String {
    let bookmark = follower::bookmark(home).await;
    let mut rows = Vec::new();
    for table in wire::TABLES {
        rows.push(format!("{}={:?}", table, follower::rows(home, table).await));
    }
    format!("bookmark={bookmark}; {}", rows.join(";"))
}

/// Everything a refusal must leave untouched: bookmark, per-table rows,
/// the subscription catalogue row, and the installation receipt state.
pub async fn state_fingerprint(home: &ShamirDb) -> String {
    let bookmark = follower::bookmark(home).await;
    let mut rows = Vec::new();
    for table in wire::TABLES {
        rows.push(format!("{}={:?}", table, follower::rows(home, table).await));
    }
    let status = follower::status_of(home, "sub")
        .await
        .map(|entry| format!("{entry:?}"))
        .unwrap_or_default();
    format!("bookmark={bookmark}; {} ;status={status}", rows.join(";"))
}

/// A resume probe that always answers with a fixed Hello — the same shape
/// a real leader advertises, so the fence logic under test is identical.
/// (Used where a mismatch cannot be produced by a live process: a restored
/// history epoch, a post-export DDL change, a wrong publication/scope. The
/// live-process identity mismatch uses a REAL fresh leader via
/// `InProcessReplSource` in the scenario itself.)
pub struct FixedHello(pub ReplResponse);

#[async_trait]
impl ResumeProbe for FixedHello {
    async fn hello(&self, _upstream: &str) -> Result<ReplResponse, SeedRefusal> {
        Ok(self.0.clone())
    }
}

/// Install `probe` as the home's resume transport (public seam — this is
/// what the server boot path does with the authenticated wire factory).
pub fn install_probe(home: &ShamirDb, probe: Arc<dyn ResumeProbe>) {
    home.set_replication_resume_probe(probe);
}

/// Assert `resume()` fails with exactly `code` and that nothing changed.
pub async fn assert_resume_refused(home: &ShamirDb, code: &str, before: &str) {
    let error = resume(home).await.expect_err("resume must be refused");
    assert!(
        error.contains(code),
        "expected refusal {code}, got: {error}",
    );
    let after = state_fingerprint(home).await;
    assert_eq!(before, after, "state changed after refused resume");
}

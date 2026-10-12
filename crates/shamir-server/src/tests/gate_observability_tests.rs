use std::time::Duration;

use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::ShamirDb;
use shamir_tx::RepoTxGate;

use crate::observability::{record_gate_metrics, sample_gate_metrics, visibility_stall};

// Red before the bridge: required labelled series are absent. Repeated
// scrapes must observe cumulative counters, not add the same count twice.
#[test]
fn scrape_renders_repo_labels_and_absolute_replay_counters() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let gate = RepoTxGate::fresh();
    let mut guard = gate.assign_next_version_guarded();
    guard.wal_accepted();
    let mut snapshot = gate.diagnostics();
    snapshot.orphan_replay_failures_total = 2;
    metrics::with_local_recorder(&recorder, || {
        record_gate_metrics("db", "repo-a", &snapshot);
        record_gate_metrics("db", "repo-a", &snapshot);
        record_gate_metrics("db", "repo-b", &RepoTxGate::fresh().diagnostics());
    });
    let rendered = handle.render();
    for name in [
        "shamir_gate_allocation_high_water",
        "shamir_gate_visible_floor",
        "shamir_gate_last_committed",
        "shamir_gate_durable_watermark",
        "shamir_gate_requires_reopen",
        "shamir_gate_visibility_waiters",
        "shamir_gate_oldest_blocker_age_seconds",
        "shamir_gate_oldest_blocker_class",
        "shamir_gate_unsettled_versions",
        "shamir_orphan_replay_failures_total",
    ] {
        for repo in ["repo-a", "repo-b"] {
            assert!(
                rendered.lines().any(|line| {
                    line.starts_with(name) && line.contains(&format!("repo=\"{repo}\""))
                }),
                "missing {name} for {repo}: {rendered}"
            );
        }
    }
    assert!(
        rendered.lines().any(|line| {
            line.starts_with("shamir_orphan_replay_failures_total{")
                && line.contains("repo=\"repo-a\"")
                && line.ends_with(" 2")
        }),
        "counter duplicated: {rendered}"
    );
    assert!(
        rendered.lines().any(|line| {
            line.starts_with("shamir_gate_oldest_blocker_class{")
                && line.contains("class=\"active_tail\"")
                && line.contains("repo=\"repo-a\"")
                && line.ends_with(" 1")
        }),
        "active class absent: {rendered}"
    );
}

// Red before operator status: a genuinely aged blocker has no status text.
#[tokio::test(start_paused = true)]
async fn operator_status_uses_lazy_age_and_clears_when_visibility_advances() {
    let gate = RepoTxGate::fresh();
    let mut guard = gate.assign_next_version_guarded();
    guard.wal_accepted();
    assert_eq!(visibility_stall("db", "repo", &gate.diagnostics()), None);
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(visibility_stall("db", "repo", &gate.diagnostics()), None);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        visibility_stall("db", "repo", &gate.diagnostics()).unwrap(),
        "db/repo: visibility stalled by version 1 (active_tail, 31.0s)",
    );
    guard.commit();
    assert_eq!(visibility_stall("db", "repo", &gate.diagnostics()), None);
}

// Exercises the actual open-repository registry used by HTTP scrapes,
// including a repository added after the recorder was constructed.
#[tokio::test]
async fn scrape_samples_current_open_registry_without_background_polling() {
    let db = ShamirDb::init_memory().await.unwrap();
    let instance = db.create_db("diagnostics-db").await;
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    instance
        .add_repo(RepoConfig::new("live", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    let repo = instance.get_repo("live").unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let guard = gate.assign_next_version_guarded();
    metrics::with_local_recorder(&recorder, || sample_gate_metrics(&db));
    let first = handle.render();
    assert!(
        first.lines().any(|line| {
            line.starts_with("shamir_gate_allocation_high_water{")
                && line.contains("db=\"diagnostics-db\"")
                && line.contains("repo=\"live\"")
                && line.ends_with(" 1")
        }),
        "open repo absent: {first}"
    );
    drop(guard);
    metrics::with_local_recorder(&recorder, || sample_gate_metrics(&db));
    let second = handle.render();
    assert!(
        second.lines().any(|line| {
            line.starts_with("shamir_gate_visible_floor{")
                && line.contains("repo=\"live\"")
                && line.ends_with(" 1")
        }),
        "scrape did not refresh: {second}"
    );
}

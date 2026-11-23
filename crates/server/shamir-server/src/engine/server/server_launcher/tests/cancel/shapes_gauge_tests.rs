use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::query::Query;
use shamir_query_types::admin::shape_stats::ResetShapeStatsReq;
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};

use crate::config::{ListenerKind, ProfileKind};
use crate::db_handler::NodeMode;
use crate::server::{BootstrapMode, ServerLauncher};

use super::shape_gauge_support::{
    boot_config, connect_cancel_client, http_get, on_big_stack, setup_cancel_table, CALL_TIMEOUT,
    CANCEL_ADMIN, CANCEL_DB, CANCEL_PASSWORD,
};

/// Parse the `shamir_shapes_tracked{...}` series carrying both `db` and
/// `repo` labels, and return its trailing value.
///
/// The exporter's label ORDER is not part of the contract, so the two
/// label substrings are matched independently; only the token after the
/// last space is parsed. A missing series is a failure of the engine
/// export path itself (the family is registered zero-touch and
/// `record_engine_metrics` re-sets it on every scrape for every open
/// repository).
fn shapes_tracked_value(metrics: &str, db: &str, repo: &str) -> u64 {
    let db_label = format!("db=\"{db}\"");
    let repo_label = format!("repo=\"{repo}\"");
    for line in metrics.lines() {
        if !line.starts_with("shamir_shapes_tracked{") {
            continue;
        }
        if !line.contains(&db_label) || !line.contains(&repo_label) {
            continue;
        }
        let value = line
            .rsplit_once(' ')
            .map(|(_, value)| value)
            .unwrap_or_default();
        return value.trim().parse().unwrap_or_else(|e| {
            panic!("shamir_shapes_tracked {db}/{repo}: bad value {value:?}: {e}")
        });
    }
    panic!(
        "shamir_shapes_tracked{{db=\"{db}\",repo=\"{repo}\"}} missing from /metrics — \
         the per-repository engine family failed to render"
    );
}

#[test]
fn shapes_tracked_gauge_counts_distinct_shapes_until_admin_reset() {
    on_big_stack(|| Box::pin(run_shapes_tracked_gauge()));
}

/// P2 oracle for `shamir_shapes_tracked{db=…,repo=…}` end to end through
/// the REAL `/metrics` route of a REAL production boot:
///
/// * `0` before any data-op traffic,
/// * exactly `2` after three real reads — two differing only in a literal
///   (ONE folded shape) and one filtering a different field (a SECOND),
/// * `0` again after `Client::reset_shape_stats` (the real admin method,
///   over the real SCRAM session, gated on `ActivityView` + `Manage` which
///   the bootstrapped superuser satisfies).
///
/// The reads deliberately filter two field names; shape folding keys a
/// filter by the field's INTERNED id and folds unknown names into one
/// shared key, so the two field names are interned first through DDL-only
/// single-field indexes. Index DDL is an admin op and records no data
/// shape, which keeps the "0 before traffic" baseline honest. Reads 2 and 3
/// differ only in the filtered FIELD (same literal value and type); a fold
/// that dropped the interned field id would merge them and the gauge would
/// read 1 instead of 2.
async fn run_shapes_tracked_gauge() {
    let guard = FlakeGuard::with_watchdog(
        "shapes_tracked_gauge_counts_distinct_shapes_until_admin_reset",
        Duration::from_secs(600),
    );
    try_install_global_tracing_ring();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let temp = tempfile::tempdir().expect("persistent data directory");
    let mut config = boot_config(temp.path().to_path_buf());
    config.argon2_concurrent_max = 64;
    config.security.auth_init_rate_per_second = 64;
    let listener = &mut config.listeners[0];
    listener.kind = ListenerKind::Tcp;
    listener.profile = Some(ProfileKind::TlsExporter);
    listener.http = Default::default();
    let launcher = ServerLauncher {
        config,
        bootstrap: BootstrapMode::Password {
            username: CANCEL_ADMIN.into(),
            password: zeroize::Zeroizing::new(CANCEL_PASSWORD.to_vec()),
        },
        node_mode: NodeMode::ReadWrite,
    };
    let mut handle = tokio::time::timeout(CALL_TIMEOUT, Box::pin(launcher.launch()))
        .await
        .expect("launch deadline")
        .expect("production boot");
    let addr = handle
        .first_tls_exporter_addr()
        .expect("bound TLS listener");
    let obs_addr = handle
        .activity_test_seam
        .as_ref()
        .expect("boot activity seam")
        .observability_bound_addr;
    assert_eq!(obs_addr.ip(), std::net::IpAddr::from([127, 0, 0, 1]));
    assert_ne!(
        obs_addr.port(),
        0,
        "config said port 0; the server must have bound a real one"
    );

    let admin = connect_cancel_client(addr, None).await;
    setup_cancel_table(&admin).await;

    // INTERN the two field names the reads filter on. Unknown field names
    // fold to one shared shape key, so without this both filters would
    // collapse to a single shape and the "2 distinct shapes" oracle would
    // be untestable. `create_index` interns every field-path segment and is
    // an admin op, so it records no data shape.
    for (index, field) in [("shapes_id_idx", "id"), ("shapes_v_idx", "v")] {
        let mut batch = Batch::new();
        batch.create_index(index, ddl::create_index(index, "items").field(field));
        tokio::time::timeout(CALL_TIMEOUT, admin.execute(CANCEL_DB, batch.build()))
            .await
            .expect("create-index DDL deadline")
            .expect("create-index DDL");
    }
    guard.mark("server, db, repo, table and shape-field indexes ready");

    // (1) No data-op traffic yet: the per-repository registry is empty and
    // the REAL `/metrics` scrape renders the gauge at zero.
    let metrics = http_get(obs_addr, "/metrics").await;
    assert_eq!(
        shapes_tracked_value(&metrics, CANCEL_DB, "main"),
        0,
        "a fresh repository must report zero tracked shapes"
    );
    guard.note("scrape", "pre-traffic /metrics fetched");

    // (2) Real reads over the real client connection: reads 2 and 3 differ
    // only in the filtered FIELD (same literal value and type); a fold that
    // dropped the interned field id would merge them and the gauge would
    // read 1 instead of 2. Read 1 differs only in its literal (folds to the
    // same shape as read 2).
    let reads = [
        (
            "literal_a",
            Query::with_repo("main", "items").where_eq("v", 1i64),
        ),
        (
            "literal_b",
            Query::with_repo("main", "items").where_eq("v", 2i64),
        ),
        (
            "other_field",
            Query::with_repo("main", "items").where_eq("id", 2i64),
        ),
    ];
    for (alias, query) in reads {
        let mut batch = Batch::new();
        batch.query(alias, query);
        let reply = tokio::time::timeout(CALL_TIMEOUT, admin.execute(CANCEL_DB, batch.build()))
            .await
            .expect("read deadline")
            .expect("read must succeed");
        let result = reply.results.get(alias).expect("read result");
        assert!(!result.skipped, "read {alias} must execute");
    }
    guard.mark("three real reads executed");

    // (3) Exactly the two distinct shapes are tracked. The count is
    // exported on the very next scrape (shape recording completes before
    // the read reply), so no waiting is involved.
    let metrics = http_get(obs_addr, "/metrics").await;
    assert_eq!(
        shapes_tracked_value(&metrics, CANCEL_DB, "main"),
        2,
        "two literal variants plus one different filter field must fold to exactly two shapes"
    );
    guard.note("scrape", "post-traffic /metrics fetched");

    // (4) The real admin method wipes the same registry the scrape reads.
    // The connecting account is the bootstrapped superuser, so the
    // `ActivityView` + `Manage` gate is satisfied.
    tokio::time::timeout(
        CALL_TIMEOUT,
        admin.reset_shape_stats(
            CANCEL_DB,
            ResetShapeStatsReq {
                repo: "main".into(),
            },
        ),
    )
    .await
    .expect("reset_shape_stats deadline")
    .expect("reset_shape_stats must be authorized and acknowledged");
    guard.mark("shape stats reset through the real client method");

    // (5) The next scrape renders the wiped registry.
    let metrics = http_get(obs_addr, "/metrics").await;
    assert_eq!(
        shapes_tracked_value(&metrics, CANCEL_DB, "main"),
        0,
        "reset_shape_stats must drop the exported tracked-shape count to zero"
    );
    guard.note("scrape", "post-reset /metrics fetched");

    tokio::time::timeout(CALL_TIMEOUT, admin.close())
        .await
        .expect("admin close deadline");
    drop(handle.activity_test_seam.take());
    let report = tokio::time::timeout(CALL_TIMEOUT, Box::pin(handle.shutdown()))
        .await
        .expect("shutdown deadline");
    assert!(report.is_clean(), "shutdown must be clean: {report:?}");
    guard.mark("shutdown complete");
    drop(temp);
}

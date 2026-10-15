//! 386-a: server-side execution of replication DDL — round-trip CRUD + list
//! against an in-memory `ShamirDb`. Queries are built ONLY through the
//! `shamir_query_builder::ddl::replication::*` builders (builder-only rule).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, drop_publication, drop_subscription, list_publications, list_subscriptions,
    publication, repl_scope, replication_profile, replication_status, subscription,
};
use shamir_query_types::admin::{ReplDirection, ReplMode};

use crate::types::value::QueryValue;
use crate::ShamirDb;

async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    // `execute` resolves the target db before dispatching admin ops; the
    // repl catalogue itself lives in the `system` repo, but the call needs a
    // valid db context.
    shamir.create_db("testdb").await;
    shamir
}

/// Extract the single admin-result record value for `alias`.
fn result_value(resp: &crate::query::batch::BatchResponse, alias: &str) -> QueryValue {
    resp.results[alias].records[0].as_value().into_owned()
}

#[tokio::test]
async fn create_publication_then_list_contains_it() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "cp",
        publication("pub_a").scope(repl_scope("app").repo("main").build()),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.op("lp", list_publications());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let val = result_value(&resp, "lp");
    let pubs = val.get("publications").and_then(|v| v.as_array()).unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0].get("name").and_then(|v| v.as_str()), Some("pub_a"));
}

#[tokio::test]
async fn create_subscription_stores_fields_and_lists() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.op("ls", list_subscriptions());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let val = result_value(&resp, "ls");
    let subs = val.get("subscriptions").and_then(|v| v.as_array()).unwrap();
    assert_eq!(subs.len(), 1);
    let s = &subs[0];
    assert_eq!(s.get("name").and_then(|v| v.as_str()), Some("sub_a"));
    assert_eq!(
        s.get("upstream").and_then(|v| v.as_str()),
        Some("tcp://leader:9000")
    );
    assert_eq!(s.get("publication").and_then(|v| v.as_str()), Some("pub_a"));
    assert_eq!(s.get("profile").and_then(|v| v.as_str()), Some("profile_a"));
    assert_eq!(s.get("state").and_then(|v| v.as_str()), Some("active"));
}

#[tokio::test]
async fn alter_subscription_pause_resume_set_profile() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // pause → state paused
    let mut b = Batch::new();
    b.id(2);
    b.try_op("alt", alter_subscription("sub_a").pause())
        .unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        sub_field(&shamir, "sub_a", "state").await,
        Some("paused".into())
    );

    // resume → state active
    let mut b = Batch::new();
    b.id(3);
    b.try_op("alt", alter_subscription("sub_a").resume())
        .unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        sub_field(&shamir, "sub_a", "state").await,
        Some("active".into())
    );

    // set_profile → profile updated
    let mut b = Batch::new();
    b.id(4);
    b.try_op("alt", alter_subscription("sub_a").set_profile("profile_b"))
        .unwrap();
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        sub_field(&shamir, "sub_a", "profile").await,
        Some("profile_b".into())
    );
}

#[tokio::test]
async fn drop_publication_and_subscription_remove_from_list() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op("cp", publication("pub_a").scope(repl_scope("app").build()));
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.op("dp", drop_publication("pub_a"));
    b.op("ds", drop_subscription("sub_a"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(3);
    b.op("lp", list_publications());
    b.op("ls", list_subscriptions());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let pubs = result_value(&resp, "lp");
    assert!(pubs
        .get("publications")
        .and_then(|v| v.as_array())
        .unwrap()
        .is_empty());
    let subs = result_value(&resp, "ls");
    assert!(subs
        .get("subscriptions")
        .and_then(|v| v.as_array())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn create_replication_profile_persists_streams() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "crp",
        replication_profile("cluster").stream(
            repl_scope("app").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Read the stored record directly from the system store.
    let table = shamir
        .system_store()
        .replication_profiles_table()
        .await
        .unwrap();
    let interner = table.interner().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    let query = crate::query::read::ReadQuery::new("replication_profiles").filter(
        crate::query::filter::Filter::Eq {
            field: vec!["name".to_string()],
            value: crate::query::filter::FilterValue::String("cluster".to_string()),
        },
    );
    let result = table.read(&query, &ctx).await.unwrap();
    assert_eq!(result.records.len(), 1);
    let rec = result.records[0].as_value();
    assert_eq!(rec.get("name").and_then(|v| v.as_str()), Some("cluster"));
    let streams = rec.get("streams").and_then(|v| v.as_array()).unwrap();
    assert_eq!(streams.len(), 1);
}

/// RI-10 Part 2: `mark_subscription_resync_required` (called by the
/// follower-loop supervisor on a terminal `JournalGap`) persists
/// `state = "resync_required"` on the subscription's row, and that value
/// round-trips verbatim through BOTH `ListSubscriptions` and
/// `ReplicationStatus` — the pre-existing admin surface that already echoes
/// `state` (no new endpoint was added). The recovery leg follows the REAL
/// procedure: every admin action on the quarantined row is refused (typed
/// `resync_required`), then DROP + replacement-seed install via the real
/// installer + `Resume` recovers it back to `"active"`.
#[tokio::test]
async fn resync_required_round_trips_through_status_and_list_then_resume_recovers() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Simulate what the supervisor does when `run_follower_loop` returns a
    // terminal `ReplError::JournalGap`.
    shamir
        .mark_subscription_resync_required("sub_a", 100, 42)
        .await
        .unwrap();
    assert_eq!(
        sub_field(&shamir, "sub_a", "state").await,
        Some("resync_required".into())
    );

    // `ListSubscriptions` echoes it verbatim.
    let mut b = Batch::new();
    b.id(2);
    b.op("ls", list_subscriptions());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let subs = result_value(&resp, "ls");
    let subs = subs
        .get("subscriptions")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(
        subs[0].get("state").and_then(|v| v.as_str()),
        Some("resync_required")
    );

    // `ReplicationStatus` echoes it verbatim too.
    let mut b = Batch::new();
    b.id(3);
    b.op("rs", replication_status());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let status = result_value(&resp, "rs");
    let entries = status
        .get("subscriptions")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].get("name").and_then(|v| v.as_str()),
        Some("sub_a")
    );
    assert_eq!(
        entries[0].get("state").and_then(|v| v.as_str()),
        Some("resync_required")
    );

    // S5c-X PR8b (revised): the quarantine is a durable marker — ANY admin
    // action on the quarantined row is refused until a replacement seed is
    // installed: Resume, Pause and SetProfile all fail with the typed code.
    for (alias, action) in [
        ("alt_resume", alter_subscription("sub_a").resume()),
        ("alt_pause", alter_subscription("sub_a").pause()),
    ] {
        let mut b = Batch::new();
        b.id(4);
        b.try_op(alias, action).unwrap();
        let error = shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("resync_required"), "{error}");
        assert_eq!(
            sub_field(&shamir, "sub_a", "state").await,
            Some("resync_required".into()),
            "refusal must not mutate the row"
        );
    }

    // Real recovery procedure (S5c-X): DROP the quarantined subscription,
    // install the replacement seed through the REAL installer (fresh paused
    // row bound to the NEW receipt), then Resume — which now succeeds.
    let mut b = Batch::new();
    b.id(5);
    b.op("drop", drop_subscription("sub_a"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    #[cfg(unix)]
    seed_recovery::reinstall_and_resume(&shamir).await;
}

/// Unix-only installer leg of
/// `resync_required_round_trips_through_status_and_list_then_resume_recovers`:
/// `SeedImageWriter` (and thus the whole seed-install path) refuses non-Unix
/// targets, so the successful-recovery half compiles out on Windows/macOS
/// (the refusal half above still runs everywhere).
#[cfg(unix)]
mod seed_recovery {
    use std::sync::Arc;

    use async_trait::async_trait;
    use shamir_engine::meta::repo_identity::RepoIdentity;
    use shamir_engine::repo::{BoxRepoFactory, SeedImageWriter, SeedReceipt, SeedTable};
    use shamir_query_builder::batch::Batch;
    use shamir_query_types::admin::{
        CreatePublicationOp, CreateSubscriptionOp, ReplDirection, ReplMode, ReplScope, ReplStream,
    };
    use shamir_query_types::wire::repl::{ReplRepoInfo, ReplResponse};

    use crate::engine::repo::RepoConfig;
    use crate::{ResumeProbe, SeedRefusal, ShamirDb};

    use super::sub_field;

    struct HelloProbe(ReplResponse);

    #[async_trait]
    impl ResumeProbe for HelloProbe {
        async fn hello(&self, _: &str) -> Result<ReplResponse, SeedRefusal> {
            Ok(self.0.clone())
        }
    }

    fn replacement_receipt() -> SeedReceipt {
        SeedReceipt {
            artifact_digest: "a".repeat(64),
            source_db: "seedtest".into(),
            source_repo: "main".into(),
            source_identity: RepoIdentity {
                lineage_id: [1; 16],
                history_epoch: 2,
            },
            schema_digest: "b".repeat(64),
            source_version: 1,
            local_baseline: 1,
        }
    }

    /// An upstream Hello consistent with [`replacement_receipt`] and the
    /// fresh (bookmark 0) destination: no gap horizon, settled at the
    /// receipt's `source_version`.
    fn matching_hello() -> ReplResponse {
        ReplResponse::Hello {
            leader_epoch: 1,
            repos: vec![ReplRepoInfo {
                db: "seedtest".into(),
                repo: "main".into(),
                current_version: 1,
                journal_floor: 0,
                repo_lineage_id: Some([1; 16]),
                history_epoch: Some(2),
                gap_first: None,
                gap_through: None,
                schema_digest: Some("b".repeat(64)),
                settled_version: Some(1),
                publications: vec![CreatePublicationOp {
                    create_publication: "pub_a".into(),
                    scopes: vec![ReplScope {
                        db: "seedtest".into(),
                        repo: Some("main".into()),
                        table: None,
                    }],
                }],
            }],
        }
    }

    pub(super) async fn reinstall_and_resume(shamir: &ShamirDb) {
        // Fresh staging destination for the replacement seed.
        shamir
            .create_db("seedtest")
            .await
            .add_repo(RepoConfig::new("main", BoxRepoFactory::in_memory()))
            .await
            .unwrap();
        let repo = shamir.get_db("seedtest").unwrap().get_repo("main").unwrap();
        let receipt = replacement_receipt();
        let writer = SeedImageWriter::begin(
            repo.clone(),
            receipt.clone(),
            vec![SeedTable {
                name: "users".into(),
                live_records: 0,
            }],
        )
        .await
        .unwrap();
        writer.finish(&[]).await.unwrap();

        // REAL installer: profile (exact streams) + paused subscription bound
        // to the new receipt.
        let streams = vec![ReplStream {
            scope: ReplScope {
                db: "seedtest".into(),
                repo: Some("main".into()),
                table: None,
            },
            direction: ReplDirection::Pull,
            mode: ReplMode::ReadOnly,
        }];
        shamir
            .install_seed_profile("profile_a", &streams)
            .await
            .unwrap();
        shamir
            .install_seed_subscription(
                &repo,
                &receipt,
                &CreateSubscriptionOp {
                    create_subscription: "sub_a".into(),
                    upstream: "tcp://leader:9000".into(),
                    publication: "pub_a".into(),
                    profile: "profile_a".into(),
                    paused: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            sub_field(shamir, "sub_a", "state").await,
            Some("paused".into())
        );

        // Validated Resume over the fresh paused row succeeds.
        shamir.set_replication_resume_probe(Arc::new(HelloProbe(matching_hello())));
        let mut b = Batch::new();
        b.id(6);
        b.try_op(
            "alt",
            shamir_query_builder::ddl::alter_subscription("sub_a").resume(),
        )
        .unwrap();
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        assert_eq!(
            sub_field(shamir, "sub_a", "state").await,
            Some("active".into())
        );
        assert_eq!(sub_field(shamir, "sub_a", "quarantined").await, None);
    }
}

/// `mark_subscription_resync_required` on a subscription name that does not
/// exist is a no-op (does not error) — the subscription may have been
/// dropped concurrently by an admin command racing with gap detection.
#[tokio::test]
async fn mark_resync_required_on_missing_subscription_is_a_noop() {
    let shamir = setup().await;
    shamir
        .mark_subscription_resync_required("does_not_exist", 100, 42)
        .await
        .expect("missing subscription must not error the caller");
}

/// S5c-X round 11: marking a subscription whose profile cannot be resolved
/// must NOT fail — quarantine is a safety action, so the persisted
/// `resync_required` state and the durable `quarantined` marker are written
/// regardless, and the read gate fails closed over the WIDEST safe scope.
/// Subscription rows record no db column (`handle_create_subscription`
/// persists only name/upstream/publication/profile/state), so the widest
/// scope is the global ("*", "*") sentinel: EVERY repo of EVERY db is
/// refused. The resolution failure is logged and surfaced via the row's
/// `status_message`, never returned as a marking error.
#[tokio::test]
async fn resync_marking_with_unresolvable_profile_fails_closed_globally() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "profile_a"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // `profile_a` was never created — profile resolution cannot succeed.
    shamir
        .mark_subscription_resync_required("sub_a", 100, 42)
        .await
        .expect("marking must never fail because the covered set is uncomputable");

    // Persisted state + durable marker were written regardless.
    assert_eq!(
        sub_field(&shamir, "sub_a", "state").await,
        Some("resync_required".into())
    );
    let table = shamir.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    let row = table
        .read(
            &crate::query::read::ReadQuery::new("subscriptions").filter(
                crate::query::filter::Filter::Eq {
                    field: vec!["name".to_string()],
                    value: crate::query::filter::FilterValue::String("sub_a".to_string()),
                },
            ),
            &ctx,
        )
        .await
        .unwrap()
        .records
        .remove(0)
        .as_value()
        .into_owned();
    assert_eq!(row.get("quarantined"), Some(&QueryValue::Bool(true)));
    let status_message = row
        .get("status_message")
        .and_then(|v| v.as_str())
        .expect("status_message set")
        .to_string();
    assert!(
        status_message.contains("profile resolution failed")
            && status_message.contains("profile_a"),
        "status_message must surface the resolution failure: {status_message}"
    );

    // Reads of the FALLBACK scope are refused with the typed code — for ANY
    // (db, repo), since no db is recorded on the row (sentinel scope).
    // The gate in changelog.rs fires BEFORE the db/repo lookup, so an
    // enforced sentinel quarantine yields Err(ResyncRequired) for ANY pair
    // — Ok(None) would be the ungated missing-repo result, i.e. a regression.
    for (db, repo) in [("testdb", "any_repo"), ("otherdb", "other")] {
        let outcome = shamir.read_changelog_bounded(db, repo, 0, 10).await;
        let error = match outcome {
            Err(error) => error,
            Ok(_) => panic!(
                "sentinel-scope quarantine must refuse the read (fail closed) \
                     for {db}/{repo}: global (\"*\", \"*\") fallback missing"
            ),
        };
        assert_eq!(
            error.code(),
            Some("resync_required"),
            "read of {db}/{repo} must be refused with the typed quarantine code"
        );
    }

    // status/list show the quarantined state.
    let mut b = Batch::new();
    b.id(2);
    b.op("ls", list_subscriptions());
    b.op("rs", replication_status());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    for alias in ["ls", "rs"] {
        let value = result_value(&resp, alias);
        let entry = &value
            .get("subscriptions")
            .and_then(|v| v.as_array())
            .unwrap()[0];
        assert_eq!(
            entry.get("state").and_then(|v| v.as_str()),
            Some("resync_required")
        );
        assert!(entry
            .get("status_message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .contains("profile resolution failed"));
    }
}

/// S5c-X 2b: publishing an EPHEMERAL backend (in-memory repo, no persistent
/// WAL) requires an explicit `replication_serve: completed_reseed` durability
/// override. A plain in-memory repo + publication fails with the typed code
/// `ephemeral_backend_requires_completed_reseed` and leaves NO publication
/// row (the check runs before any store write); a second in-memory repo with
/// the explicit override publishes fine.
#[tokio::test]
async fn publication_over_ephemeral_backend_requires_completed_reseed() {
    use shamir_query_builder::ddl::create_repo;
    use shamir_query_types::durability::{DurabilityOverride, ReplicationServe};

    let shamir = setup().await;
    // A fn item (not a closure) so the returned `&str` borrows from the argument.
    fn err_code(error: &crate::query::batch::BatchError) -> Option<&str> {
        match error {
            crate::query::batch::BatchError::QueryError { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    // Plain in-memory repo under the default durable policy.
    let mut b = Batch::new();
    b.id(1);
    b.op("cr", create_repo("r1"));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Publication covering it → typed refusal.
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "cp",
        publication("pub_x").scope(repl_scope("testdb").repo("r1").build()),
    );
    let error = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(
        err_code(&error),
        Some("ephemeral_backend_requires_completed_reseed"),
        "error: {error}"
    );
    assert!(error.to_string().contains("testdb/r1"), "{error}");
    assert!(
        error.to_string().contains("completed_reseed"),
        "message must name the required explicit value: {error}"
    );

    // No partial write: the catalogue has no publication row.
    let mut b = Batch::new();
    b.id(3);
    b.op("lp", list_publications());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let pubs = result_value(&resp, "lp");
    assert!(pubs
        .get("publications")
        .and_then(|v| v.as_array())
        .unwrap()
        .is_empty());

    // Second in-memory repo with the EXPLICIT completed_reseed override.
    let mut b = Batch::new();
    b.id(4);
    b.op(
        "cr",
        create_repo("r2").durability(DurabilityOverride {
            replication_serve: Some(ReplicationServe::CompletedReseed),
            ..Default::default()
        }),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(5);
    b.op(
        "cp",
        publication("pub_y").scope(repl_scope("testdb").repo("r2").build()),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("explicit completed_reseed override must allow publication");

    let mut b = Batch::new();
    b.id(6);
    b.op("lp", list_publications());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let pubs = result_value(&resp, "lp")
        .get("publications")
        .and_then(|v| v.as_array())
        .unwrap()
        .clone();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0].get("name").and_then(|v| v.as_str()), Some("pub_y"));
}

/// S5c-X 2e: `has_resync_required_subscription_covering` — a subscription in
/// `state = "resync_required"` whose profile has a Pull stream over the repo
/// makes the predicate true for the covered (db, repo) and false for an
/// uncovered one; with no subscriptions at all it is false (the leader serve
/// path must NOT refuse subscription-free nodes).
#[tokio::test]
async fn resync_required_subscription_covering_predicate() {
    let shamir = setup().await;

    // No subscriptions → false for anything (regression guard).
    assert!(!shamir
        .has_resync_required_subscription_covering("testdb", "main")
        .await
        .unwrap());

    let mut b = Batch::new();
    b.id(1);
    b.op(
        "crp",
        replication_profile("cluster").stream(
            repl_scope("testdb").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "cs",
        subscription("sub_a", "tcp://leader:9000", "pub_a", "cluster"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // An ACTIVE subscription does not block serving.
    assert!(!shamir
        .has_resync_required_subscription_covering("testdb", "main")
        .await
        .unwrap());

    shamir
        .mark_subscription_resync_required("sub_a", 100, 42)
        .await
        .unwrap();

    // Covered repo → true; uncovered repo/db → false.
    assert!(shamir
        .has_resync_required_subscription_covering("testdb", "main")
        .await
        .unwrap());
    assert!(!shamir
        .has_resync_required_subscription_covering("testdb", "other")
        .await
        .unwrap());
    assert!(!shamir
        .has_resync_required_subscription_covering("otherdb", "main")
        .await
        .unwrap());
}

/// Read one field of the stored subscription record from the system store.
async fn sub_field(shamir: &ShamirDb, name: &str, field: &str) -> Option<String> {
    let table = shamir.system_store().subscriptions_table().await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    let query = crate::query::read::ReadQuery::new("subscriptions").filter(
        crate::query::filter::Filter::Eq {
            field: vec!["name".to_string()],
            value: crate::query::filter::FilterValue::String(name.to_string()),
        },
    );
    let result = table.read(&query, &ctx).await.unwrap();
    result.records.first().and_then(|r| {
        r.as_value()
            .get(field)
            .and_then(|v| v.as_str())
            .map(String::from)
    })
}

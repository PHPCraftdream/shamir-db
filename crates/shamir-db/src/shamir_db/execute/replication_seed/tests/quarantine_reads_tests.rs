//! S5c-X PR8b — ordinary READS of a quarantined (`resync_required`)
//! replication destination fail closed; status/DESCRIBE stay readable;
//! healthy and non-replicated repos are unaffected.

use std::sync::atomic::Ordering;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, drop_subscription, list_subscriptions, repl_scope, replication_profile,
    subscription,
};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::batch::{BatchError, BatchResponse};

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::shamir_db::execute::SKIP_READ_QUARANTINE_FOR_TEST;
use crate::shamir_db::SystemStoreConfig;
use crate::{DbError, ShamirDb};

/// In-memory ShamirDb with db "testdb", repos "main" + "side" (each with a
/// "users" table). Mirrors the fixture layout of `system_changefeed_tests`.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    db.add_repo(
        RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(crate::engine::table::TableConfig::new("users")),
    )
    .await
    .unwrap();
    db.add_repo(
        RepoConfig::new("side", BoxRepoFactory::in_memory())
            .add_table(crate::engine::table::TableConfig::new("users")),
    )
    .await
    .unwrap();
    shamir
}

/// Insert one row into `testdb/{repo}.users`.
async fn seed_row(shamir: &ShamirDb) {
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        insert("users").row(doc! { "name" => "Alice", "age" => 30 }),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// Plain SELECT against `testdb/{repo}.users`.
async fn read_users(shamir: &ShamirDb, repo: &str) -> Result<BatchResponse, BatchError> {
    let mut b = Batch::new();
    b.id(9);
    b.query("r", Query::with_repo(repo, "users"));
    shamir.execute("testdb", &b.to_request_via_msgpack()).await
}

/// Create a Pull profile + ACTIVE subscription covering `repo` (an ordinary
/// destination — no seed receipt anywhere, so `bind_seed_creation` passes),
/// then flip the subscription to `resync_required` through the REAL persisted
/// path (`mark_subscription_resync_required`, the supervisor's entry point).
/// The refresh hook rebuilds the read-quarantine cache automatically.
async fn quarantine(shamir: &ShamirDb, sub: &str, profile: &str, repo: &str) {
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "profile",
        replication_profile(profile).stream(
            repl_scope("testdb").repo(repo).build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op("sub", subscription(sub, "tcp://peer:9000", "pub", profile));
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    shamir
        .mark_subscription_resync_required(sub, 7, 3)
        .await
        .unwrap();
}

/// Admin `ALTER SUBSCRIPTION ... RESUME` through the batch path.
async fn resume_subscription(
    shamir: &ShamirDb,
    id: u64,
    sub: &str,
) -> Result<BatchResponse, BatchError> {
    let mut b = Batch::new();
    b.id(id);
    b.try_op("alt", alter_subscription(sub).resume()).unwrap();
    shamir.execute("testdb", &b.to_request_via_msgpack()).await
}

/// Admin `ALTER SUBSCRIPTION ... PAUSE` through the batch path.
async fn pause_subscription(
    shamir: &ShamirDb,
    id: u64,
    sub: &str,
) -> Result<BatchResponse, BatchError> {
    let mut b = Batch::new();
    b.id(id);
    b.try_op("alt", alter_subscription(sub).pause()).unwrap();
    shamir.execute("testdb", &b.to_request_via_msgpack()).await
}

/// Order-independent drop guard for the negative-control seam (nextest runs
/// all tests of one binary in-process).
struct SeamGuard;

impl SeamGuard {
    fn set() -> Self {
        SKIP_READ_QUARANTINE_FOR_TEST.store(true, Ordering::Relaxed);
        SeamGuard
    }
}

impl Drop for SeamGuard {
    fn drop(&mut self) {
        SKIP_READ_QUARANTINE_FOR_TEST.store(false, Ordering::Relaxed);
    }
}

/// Reads succeed before quarantine; a Get (SELECT) and a Query batch both
/// fail with code "resync_required" once the subscription row is persisted
/// as `resync_required`.
#[tokio::test]
async fn t1_read_get_query_refused_while_quarantined() {
    let shamir = setup().await;
    seed_row(&shamir).await;
    assert!(read_users(&shamir, "main").await.is_ok());

    quarantine(&shamir, "sub", "prof", "main").await;

    let err = read_users(&shamir, "main").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));
}

/// The changelog pull path refuses a quarantined repo with
/// `DbError::ResyncRequired`.
#[tokio::test]
async fn t2_changelog_pull_refused_while_quarantined() {
    let shamir = setup().await;
    quarantine(&shamir, "sub", "prof", "main").await;

    let Err(err) = shamir.read_changelog_bounded("testdb", "main", 1, 10).await else {
        panic!("changelog pull of a quarantined repo must be refused");
    };
    assert!(matches!(err, DbError::ResyncRequired { .. }));
    assert_eq!(err.code(), Some("resync_required"));
}

/// Admin status/list stays readable while quarantined and still reports
/// `state == "resync_required"`.
#[tokio::test]
async fn t3_describe_and_status_stay_readable() {
    let shamir = setup().await;
    quarantine(&shamir, "sub", "prof", "main").await;

    let mut b = Batch::new();
    b.id(3);
    b.op("ls", list_subscriptions());
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let val = resp.results["ls"].records[0].as_value().into_owned();
    let subs = val.get("subscriptions").and_then(|v| v.as_array()).unwrap();
    assert_eq!(subs.len(), 1);
    let row = &subs[0];
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some("resync_required")
    );
    assert_eq!(row.get("name").and_then(|v| v.as_str()), Some("sub"));
}

/// Unix-only recovery helpers: `SeedImageWriter::begin` (and the whole
/// seed-install path) refuses non-Unix targets, so every helper touching the
/// real installer is compiled out on Windows/macOS — the refusal-only test
/// legs below still run everywhere.
#[cfg(unix)]
mod recovery {
    use std::sync::Arc;

    use async_trait::async_trait;
    use shamir_engine::meta::repo_identity::RepoIdentity;
    use shamir_engine::repo::{SeedImageWriter, SeedReceipt, SeedTable};
    use shamir_query_builder::batch::Batch;
    use shamir_query_types::admin::{
        CreatePublicationOp, CreateSubscriptionOp, ReplDirection, ReplMode, ReplScope, ReplStream,
    };
    use shamir_query_types::wire::repl::{ReplRepoInfo, ReplResponse};

    use crate::{ResumeProbe, SeedRefusal, ShamirDb};

    struct HelloProbe(ReplResponse);

    #[async_trait]
    impl ResumeProbe for HelloProbe {
        async fn hello(&self, _: &str) -> Result<ReplResponse, SeedRefusal> {
            Ok(self.0.clone())
        }
    }

    /// The receipt the real installer binds on the replacement seed: fresh
    /// staging for `testdb/main`, lineage `[1; 16]`, epoch 2, version 1.
    fn replacement_receipt() -> SeedReceipt {
        SeedReceipt {
            artifact_digest: "a".repeat(64),
            source_db: "testdb".into(),
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
    pub(super) fn matching_hello() -> ReplResponse {
        ReplResponse::Hello {
            leader_epoch: 1,
            repos: vec![ReplRepoInfo {
                db: "testdb".into(),
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
                    create_publication: "pub".into(),
                    scopes: vec![ReplScope {
                        db: "testdb".into(),
                        repo: Some("main".into()),
                        table: None,
                    }],
                }],
            }],
        }
    }

    pub(super) fn probe() -> Arc<dyn ResumeProbe> {
        Arc::new(HelloProbe(matching_hello()))
    }

    /// The REAL recovery procedure, installer steps only (the caller DROPs
    /// the quarantined subscription first): build a fresh destination image
    /// with `SeedImageWriter` (fresh staging, `users` inventory), then run
    /// `install_seed_profile` + `install_seed_subscription` — which create a
    /// FRESH paused subscription row bound to the NEW receipt, no marker.
    pub(super) async fn install_replacement_seed(shamir: &ShamirDb, sub: &str) {
        let repo = shamir.get_db("testdb").unwrap().get_repo("main").unwrap();
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
        let streams = vec![ReplStream {
            scope: ReplScope {
                db: "testdb".into(),
                repo: Some("main".into()),
                table: None,
            },
            direction: ReplDirection::Pull,
            mode: ReplMode::ReadOnly,
        }];
        shamir.install_seed_profile("prof", &streams).await.unwrap();
        shamir
            .install_seed_subscription(
                &repo,
                &receipt,
                &CreateSubscriptionOp {
                    create_subscription: sub.to_string(),
                    upstream: "tcp://peer:9000".into(),
                    publication: "pub".into(),
                    profile: "prof".into(),
                    paused: true,
                },
            )
            .await
            .unwrap();
    }

    /// Admin `DROP SUBSCRIPTION` through the batch path.
    pub(super) async fn drop_subscription(shamir: &ShamirDb, id: u64, sub: &str) {
        let mut b = Batch::new();
        b.id(id);
        b.op("drop", shamir_query_builder::ddl::drop_subscription(sub));
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
    }
}

/// Plain `Resume` of a quarantined subscription is refused with code
/// "resync_required", so is PAUSE (the durable `quarantined` marker survives
/// state changes), and reads stay refused; the REAL recovery procedure —
/// DROP the quarantined subscription, install a replacement seed through the
/// REAL installer (fresh paused receipt-bound row, no marker), then Resume —
/// succeeds and reads return again. The recovery half is Unix-only
/// (`SeedImageWriter` refuses non-Unix); the refusal half runs everywhere.
#[tokio::test]
async fn t4_resume_requires_replacement_seed_then_succeeds() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    // No tables here: the installed seed image carries the `users` table
    // inventory.
    db.add_repo(RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    // Quarantine FIRST, on an ORDINARY destination (no seed receipt anywhere).
    quarantine(&shamir, "sub", "prof", "main").await;
    assert!(read_users(&shamir, "main").await.is_err());

    // Plain Resume refused.
    let err = resume_subscription(&shamir, 4, "sub").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));
    // Pause of the quarantined row refused too — a state flip cannot clear
    // the quarantine marker.
    let err = pause_subscription(&shamir, 5, "sub").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));
    // Reads stay refused.
    assert!(read_users(&shamir, "main").await.is_err());

    #[cfg(unix)]
    {
        // Real recovery: DROP the quarantined subscription, then install the
        // replacement seed through the REAL installer.
        recovery::drop_subscription(&shamir, 6, "sub").await;
        recovery::install_replacement_seed(&shamir, "sub").await;
        shamir.set_replication_resume_probe(recovery::probe());
        // Resume of the fresh paused receipt-bound row SUCCEEDS.
        resume_subscription(&shamir, 7, "sub").await.unwrap();
        let resp = read_users(&shamir, "main").await.unwrap();
        assert_eq!(resp.results["r"].records.len(), 0);
    }
}

/// A non-replicated repo in the same db stays readable while `main` is
/// quarantined; so does a repo whose covering subscription is "active".
#[tokio::test]
async fn t5_healthy_and_non_replicated_repos_unaffected() {
    let shamir = setup().await;
    quarantine(&shamir, "sub", "prof", "main").await;

    // "side" has no subscription covering it.
    assert!(read_users(&shamir, "side").await.is_ok());

    // A covering subscription that is still "active" does not quarantine.
    let mut b = Batch::new();
    b.id(5);
    b.op(
        "profile_side",
        replication_profile("prof_side").stream(
            repl_scope("testdb").repo("side").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "sub_side",
        subscription("sub_side", "tcp://peer:9000", "pub", "prof_side"),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert!(read_users(&shamir, "side").await.is_ok());
    assert!(read_users(&shamir, "main").await.is_err());
}

/// The quarantine survives a full drop-and-reopen cycle on the same durable
/// backing store (cache rebuilt from the persisted rows at init). The read
/// refusal after the restart is asserted on a SECOND subscription ("sub2")
/// that is still quarantined across the restart — the first one was DROPped
/// for the real recovery, so its row (and marker) is gone. After the restart
/// "sub2"'s Resume stays refused (marker persisted); on Unix the first
/// subscription is then reseeded through the REAL installer and its Resume
/// succeeds.
#[tokio::test]
async fn t6_quarantine_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta");
    let repo_path = dir.path().join("main");

    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        let db = shamir.create_db("testdb").await;
        // Durable repo so the seed-install receipt survives the reopen (the
        // post-restart Resume validates the bound receipt against it). No
        // tables: the installed seed image carries the `users` inventory.
        db.add_repo(RepoConfig::new(
            "main",
            BoxRepoFactory::fjall(repo_path.clone()),
        ))
        .await
        .unwrap();
        quarantine(&shamir, "sub", "prof", "main").await;
        // A SECOND subscription covering the same repo, quarantined too and
        // NOT dropped — it carries the read refusal across the restart.
        quarantine(&shamir, "sub2", "prof2", "main").await;
        assert!(read_users(&shamir, "main").await.is_err());

        // DROP the first subscription (real recovery step 1); the second
        // still quarantines the repo, so reads stay refused.
        let mut b = Batch::new();
        b.id(3);
        b.op("drop", drop_subscription("sub"));
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        assert!(read_users(&shamir, "main").await.is_err());
    }

    // Reopen with a bounded retry (fjall releases its lock a few ms after
    // the owning ShamirDb is dropped) — mirrors `durable_repo_tests`.
    let mut shamir = None;
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone())).await {
            Ok(opened) => {
                shamir = Some(opened);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    let shamir = shamir.expect("system store still locked after retries");

    // After the restart the still-quarantined second subscription keeps the
    // read gate closed (the `quarantined` marker is persisted, not derived).
    let err = read_users(&shamir, "main").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));

    // And the still-quarantined row's Resume stays refused after restart.
    let err = resume_subscription(&shamir, 6, "sub2").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));

    #[cfg(unix)]
    {
        // Real recovery for the FIRST subscription: the second owner must go
        // (it would compete for the same receipt), then DROP is already done
        // pre-restart, so install + Resume.
        let mut b = Batch::new();
        b.id(7);
        b.op("drop2", drop_subscription("sub2"));
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
        recovery::install_replacement_seed(&shamir, "sub").await;
        shamir.set_replication_resume_probe(recovery::probe());
        resume_subscription(&shamir, 8, "sub").await.unwrap();
        let resp = read_users(&shamir, "main").await.unwrap();
        assert_eq!(resp.results["r"].records.len(), 0);
    }
}

/// Negative control: with the test seam set, the quarantined read is SERVED
/// (stale data) — proving the read gate itself is what fails the read.
#[tokio::test]
async fn t7_negative_control_seam_serves_stale_reads() {
    let shamir = setup().await;
    seed_row(&shamir).await;
    quarantine(&shamir, "sub", "prof", "main").await;
    assert!(read_users(&shamir, "main").await.is_err());

    let _guard = SeamGuard::set();
    let resp = read_users(&shamir, "main").await.unwrap();
    assert_eq!(resp.results["r"].records.len(), 1);
}

/// A temporal (`Temporal::AsOf`) batch read of a quarantined repo is refused
/// with the typed `BatchError::ResyncRequired`, same as an ordinary read —
/// the quarantine gate sits above the temporal dispatch.
#[tokio::test]
async fn t8_temporal_as_of_read_refused_while_quarantined() {
    let shamir = setup().await;
    seed_row(&shamir).await;
    quarantine(&shamir, "sub", "prof", "main").await;

    let mut b = Batch::new();
    b.id(8);
    b.query("r", Query::with_repo("main", "users").as_of_version(1));
    let Err(err) = shamir.execute("testdb", &b.to_request_via_msgpack()).await else {
        panic!("temporal AsOf read of a quarantined repo must be refused");
    };
    assert!(matches!(err, BatchError::ResyncRequired { .. }));
    assert_eq!(err.code(), Some("resync_required"));
}

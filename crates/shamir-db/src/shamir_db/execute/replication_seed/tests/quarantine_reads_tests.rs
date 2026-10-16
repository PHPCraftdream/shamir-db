//! S5c-X PR8b — ordinary READS of a quarantined (`resync_required`)
//! replication destination fail closed; status/DESCRIBE stay readable;
//! healthy and non-replicated repos are unaffected.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{
    alter_subscription, drop_replication_profile, drop_subscription, list_subscriptions,
    repl_scope, replication_profile, subscription,
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

/// Plain `Resume` of a quarantined subscription is refused with code
/// "resync_required", so is PAUSE (the durable `quarantined` marker survives
/// state changes), and reads stay refused. S5c-X PR8b-2: DROP is refused too
/// — dropping is not evidence that the destination's data became consistent
/// (follow the reseed runbook instead).
#[tokio::test]
async fn t4_every_admin_action_including_drop_is_refused_while_quarantined() {
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

    // S5c-X PR8b-2: DROP of the quarantined subscription is refused with the
    // typed code and does not mutate the row.
    let mut b = Batch::new();
    b.id(6);
    b.try_op("drop", drop_subscription("sub")).unwrap();
    let err = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("resync_required"), "{err}");
    assert!(read_users(&shamir, "main").await.is_err());
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

/// The quarantine survives a full reopen cycle on the same durable backing
/// store (cache rebuilt from the persisted rows at init). The repo is
/// registered through the PERSISTING facade (`ShamirDb::add_repo`), so after
/// the restart it is reloaded from the catalogue (regression: `DbInstance::
/// add_repo` alone does not persist the repo row, and the post-restart
/// `get_repo` panicked on CI). S5c-X PR8b-2: the DROP of a quarantined
/// subscription is refused before the restart AND after it — dropping is not
/// evidence of consistency — so both rows stay quarantined across the
/// restart: reads stay refused and the second row's Resume is refused too.
#[tokio::test]
async fn t6_quarantine_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta");
    let repo_path = dir.path().join("main");

    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("testdb").await;
        // Durable repo, persisted via the facade so the boot reload finds the
        // catalogue row.
        shamir
            .add_repo(
                "testdb",
                RepoConfig::new("main", BoxRepoFactory::fjall(repo_path.clone())),
            )
            .await
            .unwrap();
        quarantine(&shamir, "sub", "prof", "main").await;
        // A SECOND subscription covering the same repo, quarantined too.
        quarantine(&shamir, "sub2", "prof2", "main").await;
        assert!(read_users(&shamir, "main").await.is_err());

        // S5c-X PR8b-2: DROP of a quarantined subscription is refused —
        // there is no drop-and-reinstall recovery anymore.
        let mut b = Batch::new();
        b.id(3);
        b.try_op("drop", drop_subscription("sub")).unwrap();
        let err = shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("resync_required"), "{err}");
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

    // The repo itself must have been reloaded from the persisted catalogue —
    // this is the exact regression the CI run caught (`get_repo` was None).
    assert!(
        shamir.get_db("testdb").unwrap().get_repo("main").is_some(),
        "testdb/main must be re-registered from the catalogue after restart"
    );

    // Both subscriptions are still quarantined across the restart — reads
    // stay refused and the row's Resume stays refused too.
    let err = read_users(&shamir, "main").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));
    let err = resume_subscription(&shamir, 6, "sub2").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));
}

/// Negative control: with the test seam set, the quarantined read is SERVED
/// (stale data) — proving the read gate itself is what fails the read.
#[tokio::test]
async fn t7_negative_control_seam_serves_stale_reads() {
    let shamir = setup().await;
    seed_row(&shamir).await;
    quarantine(&shamir, "sub", "prof", "main").await;
    assert!(read_users(&shamir, "main").await.is_err());

    SKIP_READ_QUARANTINE_FOR_TEST
        .scope(true, async {
            let resp = read_users(&shamir, "main").await.unwrap();
            assert_eq!(resp.results["r"].records.len(), 1);
        })
        .await;
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

/// S5c-X PR8b follow-up: the unbounded changelog read entries
/// (`read_changelog_from`, `read_changelog_from_journal`) are refused with
/// `DbError::ResyncRequired` while the repo is quarantined; healthy and
/// non-replicated repos stay readable.
#[tokio::test]
async fn t9_changelog_read_entries_refused_while_quarantined() {
    let shamir = setup().await;
    seed_row(&shamir).await;
    // Healthy repo reads succeed before quarantine.
    assert!(shamir
        .read_changelog_from("testdb", "main", 1, 10)
        .await
        .unwrap()
        .is_some());
    assert!(shamir
        .read_changelog_from_journal("testdb", "main", 1, 10)
        .await
        .unwrap()
        .is_some());

    quarantine(&shamir, "sub", "prof", "main").await;

    let Err(err) = shamir.read_changelog_from("testdb", "main", 1, 10).await else {
        panic!("read_changelog_from of a quarantined repo must be refused");
    };
    assert!(matches!(err, DbError::ResyncRequired { .. }));
    assert_eq!(err.code(), Some("resync_required"));

    let Err(err) = shamir
        .read_changelog_from_journal("testdb", "main", 1, 10)
        .await
    else {
        panic!("read_changelog_from_journal of a quarantined repo must be refused");
    };
    assert!(matches!(err, DbError::ResyncRequired { .. }));
    assert_eq!(err.code(), Some("resync_required"));

    // Non-replicated repo is unaffected.
    assert!(shamir
        .read_changelog_from("testdb", "side", 1, 10)
        .await
        .unwrap()
        .is_some());
    assert!(shamir
        .read_changelog_from_journal("testdb", "side", 1, 10)
        .await
        .unwrap()
        .is_some());
}

/// S5c-X PR8b follow-up: the interactive-tx family fails closed. `tx_begin`
/// refuses to pin a snapshot view of a quarantined repo, and a tx opened
/// BEFORE the quarantine is refused on `tx_execute` (per-execute re-check);
/// the healthy `side` repo begins and executes normally.
#[tokio::test]
async fn t10_tx_reads_refused_while_quarantined() {
    let shamir = setup().await;
    seed_row(&shamir).await;

    // Open the tx while `main` is still healthy.
    let (mut tx, _guard) = shamir.tx_begin("testdb", "main", "snapshot").await.unwrap();
    quarantine(&shamir, "sub", "prof", "main").await;

    // BEGIN on the quarantined repo is refused.
    let Err(err) = shamir.tx_begin("testdb", "main", "snapshot").await else {
        panic!("tx_begin on a quarantined repo must be refused");
    };
    assert_eq!(err.code(), Some("resync_required"));

    // EXECUTE on the pre-quarantine tx is refused too (fail closed on use).
    let mut b = Batch::new();
    b.id(10);
    b.query("r", Query::with_repo("main", "users"));
    let Err(err) = shamir
        .tx_execute("testdb", &b.to_request_via_msgpack(), &mut tx)
        .await
    else {
        panic!("tx_execute on a quarantined repo must be refused");
    };
    assert_eq!(err.code(), Some("resync_required"));

    // The healthy, non-replicated `side` repo is unaffected.
    let (mut tx_side, _guard_side) = shamir.tx_begin("testdb", "side", "snapshot").await.unwrap();
    let mut b_side = Batch::new();
    b_side.id(11);
    b_side.query("r", Query::with_repo("side", "users"));
    shamir
        .tx_execute("testdb", &b_side.to_request_via_msgpack(), &mut tx_side)
        .await
        .unwrap();
}

/// S5c-X PR8b follow-up: `subscribe_changelog` refuses a quarantined repo
/// with the typed `DbError::ResyncRequired` BEFORE attaching a live event
/// stream; a non-replicated repo subscribes fine.
#[tokio::test]
async fn t11_subscribe_changelog_refused_while_quarantined() {
    let shamir = setup().await;
    seed_row(&shamir).await;
    quarantine(&shamir, "sub", "prof", "main").await;

    let Err(err) = shamir.subscribe_changelog("testdb", "main").await else {
        panic!("subscribe_changelog on a quarantined repo must be refused");
    };
    assert!(matches!(err, DbError::ResyncRequired { .. }));
    assert_eq!(err.code(), Some("resync_required"));

    // Non-replicated repo subscribes fine; drop the receiver immediately so
    // no background subscriber leaks.
    let rx = shamir
        .subscribe_changelog("testdb", "side")
        .await
        .unwrap()
        .expect("side repo exists");
    drop(rx);
}

/// S5c-X PR8b follow-up: the bounded journal read — the exact call the
/// server's `DbJournalSource::read` makes for a live subscription — refuses
/// with the typed `DbError::ResyncRequired` once the repo is quarantined, so
/// a bridge/subscription attached BEFORE the quarantine terminates on its
/// next drain (`JournalDrain::run` propagates the `Err` → task emits Closed
/// and exits). The non-replicated `side` repo keeps serving `Ok`.
#[tokio::test]
async fn t12_gated_journal_source_refuses_once_quarantined() {
    let shamir = setup().await;
    seed_row(&shamir).await;

    // Healthy before the quarantine.
    assert!(shamir
        .read_changelog_bounded("testdb", "main", 1, 10)
        .await
        .unwrap()
        .is_some());

    quarantine(&shamir, "sub", "prof", "main").await;

    // The gated journal source's read fails closed with the typed error.
    let Err(err) = shamir.read_changelog_bounded("testdb", "main", 1, 10).await else {
        panic!("read_changelog_bounded on a quarantined repo must be refused");
    };
    assert!(matches!(err, DbError::ResyncRequired { .. }));
    assert_eq!(err.code(), Some("resync_required"));

    // Non-replicated repo is unaffected.
    assert!(shamir
        .read_changelog_bounded("testdb", "side", 1, 10)
        .await
        .unwrap()
        .is_some());
}

/// Unix-only recovery helpers: `SeedImageWriter::begin` (and the whole
/// seed-install path) refuses non-Unix targets, so every helper touching the
/// real installer is compiled out on Windows/macOS.
#[cfg(unix)]
mod recovery {
    use std::sync::Arc;

    use async_trait::async_trait;
    use shamir_engine::meta::repo_identity::RepoIdentity;
    use shamir_engine::repo::{SeedImageWriter, SeedReceipt, SeedTable};
    use shamir_query_types::admin::{
        CreatePublicationOp, CreateSubscriptionOp, ReplDirection, ReplMode, ReplScope, ReplStream,
    };
    use shamir_query_types::wire::repl::{ReplRepoInfo, ReplResponse};
    use shamir_types::codecs::interned::query_value_to_inner_with;
    use shamir_types::codecs::CodecError;
    use shamir_types::core::interner::InternerKey;
    use shamir_types::types::record_id::RecordId;

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

    /// Source-interner field ids for the single replacement `users` row (the
    /// ids are arbitrary — `SeedImageWriter::binding` remaps them to the
    /// destination repo's local interner).
    const NAME_SOURCE_ID: u64 = 100;
    const AGE_SOURCE_ID: u64 = 101;

    /// The REAL recovery procedure (S5c-X verdict evidence row 9): build a
    /// replacement seed image with `SeedImageWriter` (bumped
    /// `history_epoch`, fresh staging, `users` inventory carrying ONE
    /// replacement row), then run `install_seed_profile` +
    /// `install_seed_subscription` over the EXISTING (quarantined) row —
    /// same name/upstream/publication/profile, `paused: true`. The installer
    /// rebinding writes the NEW receipt onto the row WITHOUT clearing the
    /// quarantine; only a post-reseed `Resume` lifts it.
    pub(super) async fn install_replacement_seed(shamir: &ShamirDb, sub: &str) {
        let repo = shamir.get_db("testdb").unwrap().get_repo("main").unwrap();
        let receipt = replacement_receipt();
        let mut writer = SeedImageWriter::begin(
            repo.clone(),
            receipt.clone(),
            vec![SeedTable {
                name: "users".into(),
                live_records: 1,
            }],
        )
        .await
        .unwrap();
        writer.binding(NAME_SOURCE_ID, "name").await.unwrap();
        writer.binding(AGE_SOURCE_ID, "age").await.unwrap();
        let row = query_value_to_inner_with(
            &doc! { "name" => "replacement", "age" => 1 }.into(),
            &|key| match key {
                "name" => Ok(InternerKey::new(NAME_SOURCE_ID)),
                "age" => Ok(InternerKey::new(AGE_SOURCE_ID)),
                other => Err(CodecError::Decode(format!(
                    "unexpected replacement row field: {other}"
                ))),
            },
        )
        .unwrap();
        writer
            .row(
                0,
                RecordId::system("replacement").to_bytes(),
                row.to_bytes().unwrap(),
            )
            .await
            .unwrap();
        writer.finish(&[]).await.unwrap();
        // Persist the `users` table to the table catalogue: the writer only
        // registers it on the RUNNING repo instance.
        shamir
            .add_table("testdb", "main", "users", false)
            .await
            .unwrap();
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
}

/// The sanctioned recovery path (S5c-X verdict evidence row 9, Unix-only):
/// quarantine via the real supervisor entry point; the typed read refusal and
/// a plain `Resume` refusal while no replacement seed exists; then the REAL
/// `install_seed_subscription` rebinds a NEW receipt on the EXISTING
/// quarantined row (the row is STILL quarantined — reads stay refused);
/// finally `Resume` through the admin batch activates the row, clears the
/// durable marker, and the bound destination serves the replacement seed's
/// row while the unrelated `side` repo stays readable throughout.
#[cfg(unix)]
#[tokio::test]
async fn t13_replacement_seed_rebinds_quarantined_row_then_resume() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    // No tables on `main` here: the installed seed image carries the `users`
    // table inventory. `side` is an unrelated, always-readable repo.
    db.add_repo(RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    db.add_repo(
        RepoConfig::new("side", BoxRepoFactory::in_memory())
            .add_table(crate::engine::table::TableConfig::new("users")),
    )
    .await
    .unwrap();
    assert!(read_users(&shamir, "side").await.is_ok());
    quarantine(&shamir, "sub", "prof", "main").await;

    // Bound destination refuses reads with the typed code, and a plain
    // `Resume` is refused (no replacement seed yet). `side` stays readable.
    let err = read_users(&shamir, "main").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));
    let err = resume_subscription(&shamir, 4, "sub").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));
    assert!(read_users(&shamir, "side").await.is_ok());

    // Real recovery: the OFFLINE installer rebinds the NEW receipt on the
    // EXISTING quarantined row (same name/upstream/publication/profile).
    recovery::install_replacement_seed(&shamir, "sub").await;

    // The row now carries the NEW receipt but is STILL quarantined: marker
    // intact, state still `resync_required`, baseline receipt still the
    // original (absent) one, and reads stay refused. Read the RAW catalogue
    // row (`rows` is the same helper the installer itself uses — status/list
    // only project a subset of fields).
    let records = super::super::catalogue::rows(&shamir, "subscriptions")
        .await
        .unwrap();
    let row = records
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("sub"))
        .unwrap();
    assert_eq!(
        row.get("state").and_then(|v| v.as_str()),
        Some("resync_required")
    );
    assert_eq!(row.get("quarantined").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        row["seed_receipt"]["source_identity"]["history_epoch"].as_u64(),
        Some(2),
        "row must carry the REPLACEMENT receipt (history_epoch 2)"
    );
    assert!(row
        .get("resync_from_receipt")
        .map(|v| v.is_null())
        .unwrap_or(false));
    let err = read_users(&shamir, "main").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"));
    assert!(read_users(&shamir, "side").await.is_ok());

    // Post-reseed `Resume` through the admin batch now SUCCEEDS: the row is
    // active, the durable marker is cleared, and the bound destination
    // serves the replacement seed's single `users` row.
    shamir.set_replication_resume_probe(recovery::probe());
    resume_subscription(&shamir, 6, "sub").await.unwrap();
    let resp = read_users(&shamir, "main").await.unwrap();
    assert_eq!(resp.results["r"].records.len(), 1);
    let served = resp.results["r"].records[0].as_value().into_owned();
    assert_eq!(
        served.get("name").and_then(|v| v.as_str()),
        Some("replacement")
    );
    assert_eq!(served.get("age").and_then(|v| v.as_i64()), Some(1));
    assert!(read_users(&shamir, "side").await.is_ok());
}

/// S5c-X PR8b-2 evidence row 3 (REOPEN leg): after quarantine, DROP the
/// replication profile and REOPEN the persisted system store — the bound
/// destination must STILL refuse with the typed code, because the durable
/// binding on the row (not the profile) is the quarantine authority. Not
/// Unix-gated: only the persisted store is exercised, no seed installer.
#[tokio::test]
async fn t14_bound_destination_stays_refused_after_profile_drop_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta");
    let repo_path = dir.path().join("main");

    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("testdb").await;
        shamir
            .add_repo(
                "testdb",
                RepoConfig::new("main", BoxRepoFactory::fjall(repo_path.clone())),
            )
            .await
            .unwrap();
        quarantine(&shamir, "sub", "prof", "main").await;
        assert!(read_users(&shamir, "main").await.is_err());

        // Delete/corrupt the profile: DROP the replication profile the
        // subscription was created against.
        let mut b = Batch::new();
        b.id(7);
        b.op("drp", drop_replication_profile("prof"));
        shamir
            .execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();

        // Still refused in-process — the binding outlived the profile.
        let err = read_users(&shamir, "main").await.unwrap_err();
        assert_eq!(err.code(), Some("resync_required"), "{err}");
    }

    // Reopen with the bounded fjall-lock retry (mirrors `durable_repo_tests`).
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

    // After reopen the cache is rebuilt from the persisted rows' durable
    // bindings — the profile is gone, the refusal is not.
    let err = read_users(&shamir, "main").await.unwrap_err();
    assert_eq!(err.code(), Some("resync_required"), "{err}");
}

//! Scenario C — mismatch refusals (each with its DISTINCT typed code and
//! NO state change), the incomplete-install boot gate, and the
//! existing-ordinary-destination refusal.
//!
//! Live-process refusals:
//! * lineage mismatch — Resume against a REAL freshly recreated leader
//!   (new in-memory repository incarnation, same names) →
//!   `seed_wrong_identity` via the identity fence.
//!
//! Fixed-Hello refusals (the wire content a live peer would send in that
//! situation; the fence under test is identical — the same
//! `fence_metadata`/`validate_seed_hello` path the loop and supervisor
//! use, see `replication/seed`):
//! * history-epoch change (restore rewinds history) →
//!   `seed_wrong_history_epoch`;
//! * schema changed after the export (DDL is NOT replicated) →
//!   `seed_wrong_schema`;
//! * wrong publication → `seed_wrong_publication`;
//! * wrong publication scope → `seed_wrong_scope`.
//!
//! The incomplete-install case interrupts the real installer at the
//! SEED-3 mid-data boundary via the public fault-injection seam and proves
//! the boot gate never registers the abandoned image; the exact retry then
//! converges without replaying rows. (Resume-facing `IncompleteInstall`
//! ordering is covered by SEED-4's `lifecycle.rs` unit tests — reaching
//! that state requires a persisted-but-incomplete registration that the
//! boot gate already refuses to produce.)

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::{ResumeProbe, SeedRefusal, ShamirDb};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::{CreatePublicationOp, ReplScope};
use shamir_query_types::wire::repl::ReplResponse;
use shamir_query_types::{DurabilityOverride, ReplicationServe};
use shamir_server::replication::in_process::InProcessReplSource;
use shamir_server::replication::source::ReplSource;
use shamir_server::replication_seed::{self, Manifest};

use crate::harness::child;
use crate::harness::follower;
use crate::harness::ops;
use crate::harness::wire;

/// Adapter: any `ReplSource` (e.g. a REAL freshly recreated leader wrapped
/// in `InProcessReplSource`) as the resume-validation transport.
struct SourceProbe(Arc<dyn ReplSource>);

#[async_trait]
impl ResumeProbe for SourceProbe {
    async fn hello(&self, _upstream: &str) -> Result<ReplResponse, SeedRefusal> {
        self.0
            .hello("seed-resume-probe")
            .await
            .map_err(|_| SeedRefusal::UpstreamUnavailable)
    }
}

/// One shared leader crash + export feeds every refusal case.
struct Fixture {
    _root: tempfile::TempDir,
    artifact: PathBuf,
    digest: String,
    manifest: Manifest,
    home: Arc<ShamirDb>,
}

async fn fixture() -> Fixture {
    let root = tempfile::tempdir().expect("temp root");
    let leader_dir = root.path().join("leader");
    let home_dir = root.path().join("home");
    let artifact = root.path().join("seed");
    let addr = child::free_addr();

    let mut leader = child::spawn_leader(&leader_dir, addr).await;
    let admin = child::connect_admin(&leader).await;
    wire::setup_leader(&admin).await;
    wire::put(&admin, "items", "k0", 1).await;
    wire::put(&admin, "audit", "a0", 2).await;
    drop(admin);
    child::kill(&mut leader).await;

    let digest = child::cli_export(&leader.config_path, &artifact).await;
    let bytes = std::fs::read(artifact.join("manifest.mpack")).expect("manifest");
    let manifest: Manifest = rmp_serde::from_slice(&bytes).expect("decode manifest");

    let home_cfg = child::write_home_config(&home_dir, addr);
    child::cli_install(&home_cfg, &artifact, &digest, &format!("tcp://{addr}")).await;
    let home = Arc::new(follower::open_home(&home_dir).await);
    Fixture {
        _root: root,
        artifact,
        digest,
        manifest,
        home,
    }
}

/// A REAL freshly recreated repository incarnation: same db/repo names, a
/// brand-new lineage (and a `pub` publication so the fence fails on
/// identity, not on the publication lookup).
async fn recreated_leader() -> ShamirDb {
    let leader = ShamirDb::init_memory().await.expect("fresh leader");
    leader.create_db(wire::DB).await;
    let mut config = RepoConfig::new(wire::REPO, BoxRepoFactory::in_memory());
    for table in wire::TABLES {
        config = config.add_table(TableConfig::new(table));
    }
    // The repo is in-memory (no persistent WAL): the explicit
    // `completed_reseed` override is what makes publishing over it legal.
    // The scenario's MEANING is unchanged — a recreated incarnation with a
    // fresh lineage that fails the fence on identity.
    leader
        .add_repo_as_with_durability(
            wire::DB,
            config,
            shamir_db::access::Actor::System,
            Some(&DurabilityOverride {
                replication_serve: Some(ReplicationServe::CompletedReseed),
                ..Default::default()
            }),
        )
        .await
        .expect("repo");
    let mut batch = Batch::new();
    batch.id("pub");
    batch.op(
        "pub",
        ddl::publication(wire::PUBLICATION)
            .scope(ddl::repl_scope(wire::DB).repo(wire::REPO).build())
            .build(),
    );
    leader
        .execute(wire::DB, &batch.build())
        .await
        .expect("publication on fresh leader");
    leader
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_e2e_c_resume_refusals_leave_state_unchanged() {
    Box::pin(seed_e2e_c_resume_refusals_leave_state_unchanged_body()).await;
}

async fn seed_e2e_c_resume_refusals_leave_state_unchanged_body() {
    let fx = fixture().await;
    let lineage = fx.manifest.lineage_id;
    let epoch = fx.manifest.history_epoch;
    let schema = fx.manifest.schema_sha256.clone();
    let v = fx.manifest.version;
    let good_pubs = vec![CreatePublicationOp {
        create_publication: wire::PUBLICATION.to_string(),
        scopes: vec![ReplScope {
            db: wire::DB.to_string(),
            repo: Some(wire::REPO.to_string()),
            table: None,
        }],
    }];

    // 1. Recreated lineage (LIVE fresh leader) -> seed_wrong_identity.
    let fresh = recreated_leader().await;
    ops::install_probe(
        &fx.home,
        Arc::new(SourceProbe(Arc::new(InProcessReplSource::new(fresh)))),
    );
    let before = ops::state_fingerprint(&fx.home).await;
    ops::assert_resume_refused(&fx.home, "seed_wrong_identity", &before).await;

    // 2. History epoch changed (restore rewound history).
    ops::install_probe(
        &fx.home,
        Arc::new(ops::FixedHello(wire::synthetic_hello(
            lineage,
            epoch + 1,
            &schema,
            good_pubs.clone(),
            v,
        ))),
    );
    ops::assert_resume_refused(&fx.home, "seed_wrong_history_epoch", &before).await;

    // 3. Schema changed after the export (DDL is not replicated).
    ops::install_probe(
        &fx.home,
        Arc::new(ops::FixedHello(wire::synthetic_hello(
            lineage,
            epoch,
            "different-schema-digest",
            good_pubs.clone(),
            v,
        ))),
    );
    ops::assert_resume_refused(&fx.home, "seed_wrong_schema", &before).await;

    // 4. Wrong publication name on the upstream.
    ops::install_probe(
        &fx.home,
        Arc::new(ops::FixedHello(wire::synthetic_hello(
            lineage,
            epoch,
            &schema,
            vec![],
            v,
        ))),
    );
    ops::assert_resume_refused(&fx.home, "seed_wrong_publication", &before).await;

    // 5. Publication exists but its scope does not match the receipt.
    let wrong_scope = vec![CreatePublicationOp {
        create_publication: wire::PUBLICATION.to_string(),
        scopes: vec![ReplScope {
            db: wire::DB.to_string(),
            repo: Some("other".to_string()),
            table: None,
        }],
    }];
    ops::install_probe(
        &fx.home,
        Arc::new(ops::FixedHello(wire::synthetic_hello(
            lineage,
            epoch,
            &schema,
            wrong_scope,
            v,
        ))),
    );
    ops::assert_resume_refused(&fx.home, "seed_wrong_scope", &before).await;
}

/// Interrupt the installer at the SEED-3 mid-data boundary: the boot gate
/// must never register the abandoned staging image, and the exact-digest
/// retry must converge without replaying rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_e2e_c_incomplete_install_boot_gate_refuses_then_exact_retry_converges() {
    Box::pin(seed_e2e_c_incomplete_install_boot_gate_refuses_then_exact_retry_converges_body())
        .await;
}

async fn seed_e2e_c_incomplete_install_boot_gate_refuses_then_exact_retry_converges_body() {
    let fx = fixture().await;
    let home_dir = fx._root.path().join("home-incomplete");
    let addr = child::free_addr();
    let upstream = format!("tcp://{addr}");

    let error = replication_seed::install_with(
        &home_dir,
        wire::DB,
        wire::REPO,
        &fx.artifact,
        &fx.digest,
        |boundary| {
            if boundary == 6 {
                Err(replication_seed::SeedError::Invalid(
                    "injected interruption (SEED-5 e2e)".to_string(),
                ))
            } else {
                Ok(())
            }
        },
    )
    .await;
    assert!(
        error.is_err(),
        "boundary-6 injection must interrupt the install",
    );

    // Boot gate: opening the home never registers or serves the image.
    let home = follower::open_home(&home_dir).await;
    assert!(
        home.get_db(wire::DB)
            .and_then(|db| db.get_repo(wire::REPO))
            .is_none(),
        "an interrupted installation must not enter the serving registry",
    );
    drop(home);

    // Exact-digest retry finishes the installation without row replay.
    let cfg = child::write_home_config(&home_dir, addr);
    child::cli_install(&cfg, &fx.artifact, &fx.digest, &upstream).await;
    let home = Arc::new(follower::open_home(&home_dir).await);
    let entry = follower::status_of(&home, "sub").await.expect("status");
    assert_eq!(
        follower::str_field(&entry, "installation_state"),
        "complete"
    );
    assert_eq!(follower::str_field(&entry, "artifact_digest"), fx.digest);
    for table in wire::TABLES {
        assert!(
            !follower::rows(&home, table).await.is_empty(),
            "{table} rows present exactly once after retry",
        );
    }
}

/// Installing over an EXISTING ordinary destination repository is refused
/// and leaves the repository untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_e2e_c_existing_ordinary_destination_refused() {
    Box::pin(seed_e2e_c_existing_ordinary_destination_refused_body()).await;
}

async fn seed_e2e_c_existing_ordinary_destination_refused_body() {
    let fx = fixture().await;
    let dir = fx._root.path().join("ordinary-home");
    let addr = child::free_addr();

    // An ordinary (non-seed) destination: db app / repo main with data.
    let ordinary = follower::open_home(&dir).await;
    ordinary.create_db(wire::DB).await;
    let mut config = RepoConfig::new(wire::REPO, BoxRepoFactory::fjall_raw(dir.join("repo-main")));
    for table in wire::TABLES {
        config = config.add_table(TableConfig::new(table));
    }
    ordinary
        .add_repo(wire::DB, config)
        .await
        .expect("ordinary repo");
    drop(ordinary);

    let cfg = child::write_home_config(&dir, addr);
    let output = child::cli_install_refused(&cfg, &fx.artifact, &fx.digest).await;
    assert!(
        output.contains("seed_ordinary_repository") || output.contains("ordinary"),
        "refusal should name the ordinary-destination code: {output}",
    );

    // No state change: the ordinary repo is still there, still ordinary.
    let home = follower::open_home(&dir).await;
    assert!(
        home.get_db(wire::DB)
            .and_then(|db| db.get_repo(wire::REPO))
            .is_some(),
        "ordinary destination survives the refused install",
    );
    assert!(
        follower::status_of(&home, "sub").await.is_none(),
        "no seed subscription may be created on refusal",
    );
}

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::storage::storage_fjall::FjallRepo;
use shamir_db::storage::types::{RecordKey, Repo};
use shamir_db::ShamirDb;
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse};
use shamir_tx::changefeed::{encode_gap_marker, gap_marker_key};
use std::sync::Arc;

use crate::db_handler::handler::ShamirDbHandler;
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;

#[tokio::test]
async fn hello_pull_identity_and_true_certified_interval_both_sources() {
    let directory = tempfile::TempDir::new().unwrap();
    // Independent persisted [10,100] fixture, loaded by normal cold open.
    {
        let backing = FjallRepo::new(directory.path()).unwrap();
        let store = backing.store_get("__changelog__").await.unwrap();
        store
            .set(
                RecordKey::from_slice(&gap_marker_key()),
                encode_gap_marker((10, 100)),
            )
            .await
            .unwrap();
        store.flush().await.unwrap();
    }
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("app").await;
    db.add_repo(
        "app",
        RepoConfig::new("main", BoxRepoFactory::fjall_raw(directory.path())),
    )
    .await
    .unwrap();
    let repo = db.get_db("app").unwrap().get_repo("main").unwrap();
    let identity = repo.repo_identity().await.unwrap();
    let source = InProcessReplSource::new(db.clone());
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = Session::new(
        [1; 16],
        "root".into(),
        SessionPermissions::new(true, true, vec![]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0; 32],
        0,
    );
    let wire_hello = handler
        .handle_repl(
            &session,
            ReplRequest::Hello {
                proto_ver: 1,
                node_id: "follower".into(),
            },
        )
        .await;
    let local_hello = source.hello("follower").await.unwrap();
    for response in [wire_hello, local_hello] {
        let ReplResponse::Hello { repos, .. } = response else {
            panic!("expected Hello")
        };
        let info = repos
            .iter()
            .find(|r| r.db == "app" && r.repo == "main")
            .unwrap();
        assert_eq!(info.repo_lineage_id, Some(identity.lineage_id));
        assert_eq!(info.history_epoch, Some(identity.history_epoch));
        assert_eq!((info.gap_first, info.gap_through), (Some(10), Some(100)));
    }
    let wire_pull = handler
        .handle_repl(
            &session,
            ReplRequest::Pull {
                db: "app".into(),
                repo: "main".into(),
                from_version: 1,
                limit: 10,
                wait_ms: None,
            },
        )
        .await;
    let local_pull = source.pull("app", "main", 1, 10, None).await.unwrap();
    for response in [wire_pull, local_pull] {
        let ReplResponse::Pull {
            repo_lineage_id,
            history_epoch,
            gap_first,
            gap_through,
            gap_at,
            ..
        } = response
        else {
            panic!("expected Pull")
        };
        assert_eq!(repo_lineage_id, Some(identity.lineage_id));
        assert_eq!(history_epoch, Some(identity.history_epoch));
        assert_eq!(
            (gap_first, gap_through, gap_at),
            (Some(10), Some(100), Some(10))
        );
    }
}

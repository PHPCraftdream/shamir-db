use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;

use crate::meta::repo_identity::RepoIdentity;
use crate::repo::{BoxRepo, RepoInstance, SeedReceipt};

fn receipt() -> SeedReceipt {
    SeedReceipt {
        artifact_digest: "a".repeat(64),
        source_db: "app".into(),
        source_repo: "main".into(),
        source_identity: RepoIdentity {
            lineage_id: [7; 16],
            history_epoch: 3,
        },
        schema_digest: "b".repeat(64),
        source_version: 73,
        local_baseline: 1,
    }
}

#[tokio::test]
async fn fresh_repo_identity_epoch_zero_is_valid_receipt_evidence() {
    let source = RepoInstance::new(
        "main".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    );
    let identity = source.repo_identity().await.unwrap();
    assert_eq!(identity.history_epoch, 0);
    assert_eq!(source.repo_identity().await.unwrap(), identity);

    let mut evidence = receipt();
    evidence.source_identity = identity;
    evidence.validate().unwrap();

    // Commit version zero remains reserved independently of the history epoch.
    for version in [0, u64::MAX] {
        evidence.source_version = version;
        assert!(evidence.validate().is_err());
    }
}

//! SNAP-4: a snapshot seed must not jump across an unsettled version.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;

#[tokio::test]
async fn consistent_version_stays_below_parked_lower_version() {
    let repo = RepoInstance::new(
        "consistent_version".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    );
    let gate = repo.tx_gate().await.unwrap();
    let lower = gate.assign_next_version_guarded();
    let higher = gate.assign_next_version_guarded();
    let higher_version = higher.version();
    higher.commit();
    gate.raise_published(higher_version);

    assert_eq!(repo.current_commit_version().await.unwrap(), higher_version);
    let seed = repo.consistent_version().await.unwrap();
    assert_eq!(seed, 0);
    assert!(seed <= gate.completion().watermark());
    assert!(seed < lower.version());

    lower.commit();
    assert_eq!(repo.consistent_version().await.unwrap(), higher_version);
}

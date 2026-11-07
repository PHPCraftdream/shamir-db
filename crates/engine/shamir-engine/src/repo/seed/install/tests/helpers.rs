use std::sync::Arc;

use bytes::Bytes;
use shamir_collections::TMap;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::value::InnerValue;

use crate::meta::repo_identity::RepoIdentity;
use crate::repo::{BoxRepo, RepoInstance, SeedImageWriter, SeedReceipt, SeedTable};

pub(super) fn receipt() -> SeedReceipt {
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

pub(super) fn repo(backing: Arc<InMemoryRepo>, tables: Vec<SeedTable>) -> RepoInstance {
    RepoInstance::new(
        "main".into(),
        BoxRepo::InMemory(backing),
        tables.iter().map(SeedTable::config).collect(),
    )
}

pub(super) fn inventory() -> Vec<SeedTable> {
    vec![
        SeedTable {
            name: "items".into(),
            live_records: 1,
        },
        SeedTable {
            name: "empty".into(),
            live_records: 0,
        },
    ]
}

pub(super) fn payload() -> Bytes {
    let mut nested = TMap::default();
    nested.insert(InternerKey::new(900), InnerValue::Bin(vec![0, 255]));
    let mut root = TMap::default();
    root.insert(
        InternerKey::new(700),
        InnerValue::List(vec![InnerValue::Map(nested)]),
    );
    InnerValue::Map(root).to_bytes().unwrap()
}

pub(super) async fn populated(repo: RepoInstance) -> SeedImageWriter {
    let mut writer = SeedImageWriter::begin(repo, receipt(), inventory())
        .await
        .unwrap();
    writer.binding(700, "outer").await.unwrap();
    writer.binding(900, "inner").await.unwrap();
    writer
        .row(0, Bytes::from_static(b"opaque-key"), payload())
        .await
        .unwrap();
    writer
}

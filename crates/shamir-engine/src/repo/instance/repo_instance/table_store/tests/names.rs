use crate::repo::instance::repo_instance::table_store::table_store_names;
use crate::repo::journal::backend::repo_types::RepoFactory;
use crate::repo::repo_types::BoxRepoFactory;
#[cfg(feature = "fjall")]
use shamir_storage::storage_cached::WriteMode;
use shamir_storage::types::Repo;
use shamir_types::types::record_id::RecordId;

#[test]
fn legacy_and_incarnation_mappings_are_distinct() {
    let legacy = table_store_names("items", None);
    assert_eq!(legacy.data, "__data__items");
    assert_eq!(legacy.info, "__info__items");
    assert_eq!(legacy.history, "__history__items");
    let first = RecordId::from_ts(1_770_000_000_000_000);
    let second = RecordId::from_ts(1_770_000_000_000_001);
    let a = table_store_names("items", Some(&first));
    let b = table_store_names("items", Some(&second));
    assert_ne!(a.data, b.data);
    assert_eq!(
        &a.data[..a.data.len() - first.to_string().len()],
        &b.data[..b.data.len() - second.to_string().len()]
    );
    assert!(a.data.starts_with("__data__items--"));
    assert!(a.info.starts_with("__info__items--"));
    assert!(a.history.starts_with("__history__items--"));
}

#[test]
fn generated_names_open_in_memory_backend() {
    let names = table_store_names("items", Some(&RecordId::new()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let repo = BoxRepoFactory::in_memory().create().await.unwrap();
        for name in [names.data, names.info, names.history] {
            repo.store_get(name).await.unwrap();
        }
    });
}

#[cfg(feature = "fjall")]
#[test]
fn generated_names_open_in_fjall_backend() {
    let names = table_store_names("items", Some(&RecordId::new()));
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let repo = BoxRepoFactory::fjall(dir.path()).create().await.unwrap();
        for name in [names.data, names.info, names.history] {
            repo.store_get(name).await.unwrap();
        }
    });
}

#[cfg(feature = "fjall")]
#[test]
fn generated_names_open_in_hybrid_backend() {
    let names = table_store_names("items", Some(&RecordId::new()));
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let repo = BoxRepoFactory::hybrid(dir.path()).create().await.unwrap();
        for name in [names.data, names.info, names.history] {
            repo.store_get(name).await.unwrap();
        }
    });
}

#[cfg(feature = "fjall")]
#[test]
fn generated_names_open_in_cached_backend() {
    let names = table_store_names("items", Some(&RecordId::new()));
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let factory = BoxRepoFactory::cached(BoxRepoFactory::fjall(dir.path()), WriteMode::Sync);
        let repo = factory.create().await.unwrap();
        for name in [names.data, names.info, names.history] {
            repo.store_get(name).await.unwrap();
        }
    });
}

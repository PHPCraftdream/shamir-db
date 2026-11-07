use crate::server_meta::ServerMetaStore;
use tempfile::TempDir;

fn store_at(dir: &TempDir) -> ServerMetaStore {
    ServerMetaStore::open_or_init(dir.path().join("server_meta")).expect("open meta")
}

#[test]
fn token_key_first_open_and_reopen_preserve_exact_bytes() {
    let dir = TempDir::new().expect("tempdir");
    let key = store_at(&dir).token_key().expect("token key");
    let bytes = *key.as_bytes();
    assert_eq!(format!("{key:?}"), "TokenKey(<REDACTED>)");
    drop(key);
    assert_eq!(
        *store_at(&dir).token_key().expect("reopened key").as_bytes(),
        bytes
    );
}

#[test]
fn token_key_accessor_rejects_non_32_byte_storage() {
    let dir = TempDir::new().expect("tempdir");
    let store = store_at(&dir);
    store.set_token_key_for_test(&[1; 31]);
    assert!(store.token_key().is_err());
    store.set_token_key_for_test(&[1; 33]);
    assert!(store.token_key().is_err());
}

#[test]
fn malformed_token_key_length_fails_open_closed() {
    let dir = TempDir::new().expect("tempdir");
    let store = store_at(&dir);
    store.set_token_key_for_test(&[0; 31]);
    drop(store);
    assert!(ServerMetaStore::open_or_init(dir.path().join("server_meta")).is_err());
}

#[test]
fn legacy_meta_lazy_upgrade_preserves_other_key_bytes() {
    let dir = TempDir::new().expect("tempdir");
    let store = store_at(&dir);
    let keys = ["secrets", "audit_chain", "ticket", "identity", "bootstrap"]
        .into_iter()
        .map(|name| (name, store.blob_for_test(name).expect("existing blob")))
        .collect::<Vec<_>>();
    store.remove_token_key_for_test();
    drop(store);

    let upgraded = store_at(&dir);
    assert!(format!("{upgraded:?}").contains("token_key: \"<REDACTED>\""));
    for (name, bytes) in &keys {
        assert_eq!(upgraded.blob_for_test(name).expect("upgraded blob"), *bytes);
    }
    let upgraded_key = *upgraded.token_key().expect("upgrade key").as_bytes();
    drop(upgraded);
    assert_eq!(
        *store_at(&dir)
            .token_key()
            .expect("reopened token key")
            .as_bytes(),
        upgraded_key
    );
}

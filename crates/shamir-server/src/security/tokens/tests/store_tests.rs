use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::token::{OwnerFloors, TokenChannels, TokenId, TokenMode, TokenRecord};
use std::sync::Arc;
use tempfile::TempDir;

use super::super::service::{TokenService, TokenServiceOptions};
use super::super::TokenStore;

fn record(id: u8) -> TokenRecord {
    TokenRecord {
        id: [id; 16],
        owner_user_id: [1; 16],
        label: format!("record-{id}"),
        mac: [2; 32],
        key_id: 1,
        format_version: 1,
        created_at_ns: 1,
        expires_at_ns: 100,
        mode: TokenMode::ReadOnly,
        channels: TokenChannels::Both,
        databases: None,
        roles: Vec::new(),
        revoked_at_ns: None,
    }
}

struct KnownOwner;
impl OwnerFloors for KnownOwner {
    fn floor_if_present(&self, _user_id: &[u8; 16]) -> Option<u64> {
        Some(0)
    }
}
struct NullAudit;
impl AuditAppender for NullAudit {
    fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}
fn audit() -> Arc<AuditChainWriter> {
    Arc::new(AuditChainWriter::new(
        AuditChain::new([0; 32]),
        Arc::new(NullAudit),
    ))
}

#[tokio::test]
async fn store_put_if_absent_preserves_existing_id() {
    let temp = TempDir::new().expect("tempdir");
    let store = TokenStore::open(temp.path()).await.expect("store");
    store.put(record(3)).await.expect("seed record");
    let mut collision = record(3);
    collision.label = "changed label".to_string();
    collision.mac = [9; 32];
    assert!(!store
        .put_if_absent_blocking(&collision)
        .expect("collision check"));
    assert_eq!(
        store.get(TokenId([3; 16])).await.expect("get"),
        Some(record(3))
    );
}

#[tokio::test]
async fn token_store_round_trip_and_live_reload() {
    let temp = TempDir::new().expect("tempdir");
    let store = TokenStore::open(temp.path()).await.expect("store");
    store.put(record(1)).await.expect("put");
    assert_eq!(
        store.get(TokenId([1; 16])).await.expect("get"),
        Some(record(1))
    );
    assert_eq!(store.scan().await.expect("scan"), vec![record(1)]);
    drop(store);
    assert_eq!(
        TokenStore::open(temp.path())
            .await
            .expect("reopen")
            .scan()
            .await
            .expect("reload"),
        vec![record(1)]
    );
}

#[tokio::test]
async fn token_store_skips_and_counts_corrupt_records() {
    let temp = TempDir::new().expect("tempdir");
    let store = TokenStore::open(temp.path()).await.expect("store");
    store
        .with_locked_records(|db, ks| {
            let mut batch = db.batch().durability(Some(fjall::PersistMode::SyncAll));
            batch.insert(ks, [7; 16], b"invalid-msgpack".as_slice());
            batch.commit().expect("fixture batch");
            Ok(())
        })
        .expect("insert corrupt fixture");
    assert!(store.scan().await.expect("scan").is_empty());
    assert_eq!(store.corrupt_records_hint(), 1);
}

#[tokio::test]
async fn service_open_loads_preseeded_live_records_and_skips_non_live_corrupt() {
    let temp = TempDir::new().expect("tempdir");
    let store = TokenStore::open(temp.path()).await.expect("store");
    let now = 50_000_000_000_u64;
    let mut live = record(10);
    live.expires_at_ns = now + 1_000;
    let mut expired = record(11);
    expired.expires_at_ns = now - 1;
    let mut revoked = record(12);
    revoked.revoked_at_ns = Some(5);
    for row in [live.clone(), expired.clone(), revoked.clone()] {
        store.put(row).await.expect("preseed record");
    }
    store
        .with_locked_records(|db, ks| {
            let mut batch = db.batch().durability(Some(fjall::PersistMode::SyncAll));
            batch.insert(ks, [13; 16], b"corrupt".as_slice());
            batch.commit().expect("fixture batch");
            Ok(())
        })
        .expect("insert corrupt record");
    drop(store);
    let service = TokenService::open_with_clock(TokenServiceOptions {
        data_dir: temp.path().to_path_buf(),
        token_key: zeroize::Zeroizing::new([9; 32]),
        owners: Arc::new(KnownOwner),
        sessions: Arc::new(shamir_connect::server::TokenSessionStore::new()),
        audit: audit(),
        config: crate::config::TokensConfig::default(),
        read_only_node: true,
        clock: Arc::new(move || Some(now)),
    })
    .await
    .expect("service open");
    assert_eq!(service.mirror().len_hint(), 1);
    assert_eq!(
        service
            .mirror()
            .get(&TokenId(live.id))
            .expect("loaded live")
            .record(),
        &live
    );
    assert!(service.mirror().get(&TokenId(expired.id)).is_none());
    assert!(service.mirror().get(&TokenId(revoked.id)).is_none());
}

#[tokio::test]
async fn garbage_collection_removes_only_records_beyond_exact_retention_boundary() {
    let temp = TempDir::new().expect("tempdir");
    let store = TokenStore::open(temp.path()).await.expect("store");
    let mut expiry_boundary = record(1);
    expiry_boundary.expires_at_ns = 10;
    let mut expiry_old = record(2);
    expiry_old.expires_at_ns = 9;
    let mut revoke_boundary = record(3);
    revoke_boundary.revoked_at_ns = Some(10);
    let mut revoke_old = record(4);
    revoke_old.revoked_at_ns = Some(9);
    for row in [expiry_boundary, expiry_old, revoke_boundary, revoke_old] {
        store.put(row).await.expect("fixture");
    }
    let now = 10 + 30 * 86_400_000_000_000_u64;
    drop(store);
    let service = TokenService::open_with_clock(TokenServiceOptions {
        data_dir: temp.path().to_path_buf(),
        token_key: zeroize::Zeroizing::new([9; 32]),
        owners: Arc::new(KnownOwner),
        sessions: Arc::new(shamir_connect::server::TokenSessionStore::new()),
        audit: audit(),
        config: crate::config::TokensConfig::default(),
        read_only_node: false,
        clock: Arc::new(move || Some(now)),
    })
    .await
    .expect("service");
    assert_eq!(service.collect_garbage_now().await.expect("gc"), 0);
    drop(service);
    let remaining: Vec<u8> = TokenStore::open(temp.path())
        .await
        .expect("reopen")
        .scan()
        .await
        .expect("scan")
        .into_iter()
        .map(|row| row.id[0])
        .collect();
    assert_eq!(remaining, vec![1, 3]);
}

#[tokio::test]
async fn injected_write_failure_is_consumed_by_gc_batch_before_publication() {
    let temp = TempDir::new().expect("tempdir");
    let store = TokenStore::open(temp.path()).await.expect("store");
    drop(store);
    let service = TokenService::open_with_clock(TokenServiceOptions {
        data_dir: temp.path().to_path_buf(),
        token_key: zeroize::Zeroizing::new([9; 32]),
        owners: Arc::new(KnownOwner),
        sessions: Arc::new(shamir_connect::server::TokenSessionStore::new()),
        audit: audit(),
        config: crate::config::TokensConfig::default(),
        read_only_node: false,
        clock: Arc::new(|| Some(4_000_000_000_000_000)),
    })
    .await
    .expect("service");
    let mut expired = record(9);
    expired.expires_at_ns = 2;
    service
        .store_for_tests()
        .put(expired.clone())
        .await
        .expect("seed");
    service.store_for_tests().fail_next_durable_write();
    assert!(matches!(
        service.collect_garbage_now().await,
        Err(crate::security::tokens::TokenStoreError::InjectedWriteFailure)
    ));
    assert_eq!(
        service
            .store_for_tests()
            .get(TokenId(expired.id))
            .await
            .expect("get"),
        Some(expired)
    );
}

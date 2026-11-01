use crate::security::tokens::{
    NewToken, SessionCanceller, TokenCreateError, TokenService, TokenServiceOptions,
};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::token::{Endpoint, OwnerFloors, TokenChannels, TokenId, TokenMode};
use shamir_connect::server::TokenSessionStore;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use tempfile::TempDir;
use zeroize::Zeroizing;

struct Owners;
impl OwnerFloors for Owners {
    fn floor_if_present(&self, id: &[u8; 16]) -> Option<u64> {
        (*id == [1; 16]).then_some(0)
    }
}
struct NoopAppender;
impl AuditAppender for NoopAppender {
    fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}
struct CancelObserver {
    count: AtomicUsize,
    mirror: Arc<shamir_connect::server::token::TokenMirror>,
    store: crate::security::tokens::TokenStore,
}
impl SessionCanceller for CancelObserver {
    fn cancel_token(&self, id: &TokenId) {
        assert!(self.mirror.get(id).is_some_and(|state| state.is_revoked()));
        assert!(self
            .store
            .get_blocking(id)
            .expect("durable record")
            .is_some_and(|record| record.revoked_at_ns.is_some()));
        self.count.fetch_add(1, Ordering::SeqCst);
    }
}
fn audit() -> Arc<AuditChainWriter> {
    Arc::new(AuditChainWriter::new(
        AuditChain::new([8; 32]),
        Arc::new(NoopAppender),
    ))
}
fn new_token(label: String, ttl_secs: Option<u64>) -> NewToken {
    NewToken {
        owner_user_id: [1; 16],
        owner_roles: vec!["reader".to_string()],
        label,
        ttl_secs,
        mode: TokenMode::ReadOnly,
        channels: TokenChannels::Both,
        databases: None,
    }
}
async fn service_with_owners(
    temp: &TempDir,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    owners: Arc<dyn OwnerFloors + Send + Sync>,
) -> Result<TokenService, crate::security::tokens::TokenStoreError> {
    let options = TokenServiceOptions {
        data_dir: temp.path().to_path_buf(),
        token_key: Zeroizing::new([5; 32]),
        owners,
        sessions: Arc::new(TokenSessionStore::new()),
        audit: audit(),
        config: crate::config::TokensConfig::default(),
        read_only_node: false,
        clock,
    };
    TokenService::open_with_clock(options).await
}

async fn service(
    temp: &TempDir,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
) -> TokenService {
    service_with_owners(temp, clock, Arc::new(Owners))
        .await
        .expect("open service")
}

#[tokio::test]
async fn revoke_user_only_revokes_owners_tokens_and_leaves_floor_untouched() {
    let temp = TempDir::new().expect("tempdir");
    let now = 80_000_000_000_u64;
    let service = service(&temp, Arc::new(move || Some(now))).await;
    service
        .create(new_token("one".into(), Some(60)))
        .await
        .expect("create one");
    let mut other = new_token("other-owner".into(), Some(60));
    other.owner_user_id = [2; 16];
    service.create(other).await.expect("create other");
    assert_eq!(service.floor_for_tests(&[1; 16]), Some(0));
    assert_eq!(
        service
            .revoke_user([1; 16], now + 1)
            .await
            .expect("revoke owner"),
        1
    );
    assert_eq!(service.floor_for_tests(&[1; 16]), Some(0));
    let listed_owner = service.list([1; 16]).await.expect("owner list");
    assert!(listed_owner
        .iter()
        .all(|token| token.revoked_at_ns.is_some()));
    let listed_other = service.list([2; 16]).await.expect("other list");
    assert!(listed_other
        .iter()
        .all(|token| token.revoked_at_ns.is_none()));
    assert_eq!(
        service.verify(b"bad", Endpoint::Rest, now).unwrap_err(),
        shamir_connect::server::token::VerifyFailure::Invalid
    );
}

#[tokio::test]
async fn retained_non_live_record_is_revocable() {
    let temp = TempDir::new().expect("tempdir");
    let now = 70_000_000_000_u64;
    let service = service(&temp, Arc::new(move || Some(now))).await;
    let created = service
        .create(new_token("retained".into(), Some(1)))
        .await
        .expect("create");
    let id = TokenId(created.record_view.id);
    service.mirror().remove(&id);
    assert!(service
        .revoke(id, now + 2_000_000_000)
        .await
        .expect("retained revoke"));
    let stored = service.list([1; 16]).await.expect("list");
    assert_eq!(
        stored
            .iter()
            .find(|row| row.id == id.0)
            .expect("retained view")
            .revoked_at_ns,
        Some(now + 2_000_000_000)
    );
}

#[tokio::test]
async fn gc_task_exits_after_cancellation_without_sleeping() {
    let temp = TempDir::new().expect("tempdir");
    let now = 30_000_000_000_u64;
    let service = service(&temp, Arc::new(move || Some(now))).await;
    let stop = tokio_util::sync::CancellationToken::new();
    service.start_gc(stop.clone()).await;
    stop.cancel();
    service.shutdown_gc().await;
    assert!(service.gc_task_finished().await);
}

#[tokio::test]
async fn create_verify_and_revoke_publishes_after_durable_record() {
    let temp = TempDir::new().expect("tempdir");
    let now = 100_000_000_000_u64;
    let service = service(&temp, Arc::new(move || Some(now))).await;
    let canceller = Arc::new(CancelObserver {
        count: AtomicUsize::new(0),
        mirror: Arc::clone(service.mirror()),
        store: service.store_for_tests().clone(),
    });
    let service = service.with_canceller(canceller.clone());
    let created = service
        .create(new_token("deployment".into(), None))
        .await
        .expect("create");
    assert!(service
        .verify(created.token.as_bytes(), Endpoint::Rest, now)
        .is_ok());
    assert!(!format!("{created:?}").contains(created.token.as_str()));
    let id = TokenId(created.record_view.id);
    assert!(service.revoke(id, now + 7).await.expect("revoke"));
    assert!(matches!(
        service.verify(created.token.as_bytes(), Endpoint::Rest, now + 8),
        Err(shamir_connect::server::token::VerifyFailure::Revoked)
    ));
    assert_eq!(canceller.count.load(Ordering::SeqCst), 1);
    assert_eq!(
        service
            .revoke(id, now + 9)
            .await
            .expect("idempotent revoke"),
        false
    );
}

#[tokio::test]
async fn labels_and_limits_count_only_active_unexpired_records() {
    let temp = TempDir::new().expect("tempdir");
    let now = 1_000_000_000_u64;
    let mut config = crate::config::TokensConfig::default();
    config.max_per_user = 1;
    let service = TokenService::open_with_clock(TokenServiceOptions {
        data_dir: temp.path().to_path_buf(),
        token_key: Zeroizing::new([5; 32]),
        owners: Arc::new(Owners),
        sessions: Arc::new(TokenSessionStore::new()),
        audit: audit(),
        config,
        read_only_node: false,
        clock: Arc::new(move || Some(now)),
    })
    .await
    .expect("service");
    let created = service
        .create(new_token("slot".into(), Some(10)))
        .await
        .expect("create");
    assert!(matches!(
        service.create(new_token("slot".into(), Some(10))).await,
        Err(TokenCreateError::LabelExists)
    ));
    assert!(matches!(
        service.create(new_token("other".into(), Some(10))).await,
        Err(TokenCreateError::LimitReached)
    ));
    service
        .revoke(TokenId(created.record_view.id), now + 1)
        .await
        .expect("revoke");
    assert!(service
        .create(new_token("slot".into(), Some(10)))
        .await
        .is_ok());
}

#[tokio::test]
async fn ttl_bounds_zero_over_max_and_default_are_enforced() {
    let temp = TempDir::new().expect("tempdir");
    let now = 5_000_000_000_u64;
    let service = service(&temp, Arc::new(move || Some(now))).await;
    assert!(matches!(
        service.create(new_token("zero".into(), Some(0))).await,
        Err(TokenCreateError::TokenTtlInvalid)
    ));
    assert!(matches!(
        service
            .create(new_token("too-long".into(), Some(365 * 86_400 + 1)))
            .await,
        Err(TokenCreateError::TokenTtlInvalid)
    ));
    let created = service
        .create(new_token("default".into(), None))
        .await
        .expect("default ttl");
    assert_eq!(
        created.record_view.expires_at_ns - created.record_view.created_at_ns,
        30 * 86_400_000_000_000
    );
}

#[tokio::test]
async fn database_allowlist_shape_is_rejected_before_persistence() {
    let temp = TempDir::new().expect("tempdir");
    let now = 5_000_000_000_u64;
    let service = service(&temp, Arc::new(move || Some(now))).await;
    let mut input = new_token("bad-db".into(), Some(60));
    input.databases = Some(vec![String::new()]);
    assert!(matches!(
        service.create(input).await,
        Err(TokenCreateError::InvalidInput)
    ));
    assert_eq!(service.mirror_entry_count_hint(), 0);
}

#[tokio::test]
async fn concurrent_owner_creates_never_exceed_active_limit() {
    let temp = TempDir::new().expect("tempdir");
    let now = 20_000_000_000_u64;
    let mut config = crate::config::TokensConfig::default();
    config.max_per_user = 3;
    let service = Arc::new(
        TokenService::open_with_clock(TokenServiceOptions {
            data_dir: temp.path().to_path_buf(),
            token_key: Zeroizing::new([5; 32]),
            owners: Arc::new(Owners),
            sessions: Arc::new(TokenSessionStore::new()),
            audit: audit(),
            config,
            read_only_node: false,
            clock: Arc::new(move || Some(now)),
        })
        .await
        .expect("service"),
    );
    let barrier = Arc::new(Barrier::new(9));
    std::thread::scope(|scope| {
        for index in 0..8 {
            let service = Arc::clone(&service);
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                barrier.wait();
                let result = runtime
                    .block_on(service.create(new_token(format!("parallel-{index}"), Some(60))));
                assert!(result.is_ok() || matches!(result, Err(TokenCreateError::LimitReached)));
            });
        }
        barrier.wait();
    });
    assert_eq!(service.mirror_entry_count_hint(), 3);
}

#[tokio::test]
async fn owner_floor_rejects_unknown_and_equal_floor_timestamp() {
    let temp = TempDir::new().expect("tempdir");
    let now = 40_000_000_000_u64;
    let created = service(&temp, Arc::new(move || Some(now)))
        .await
        .create(new_token("floor".into(), Some(60)))
        .await
        .expect("create");
    assert!(
        service_with_owners(&temp, Arc::new(move || Some(now)), Arc::new(UnknownOwner))
            .await
            .expect("unknown owner service")
            .verify(created.token.as_bytes(), Endpoint::Rest, now)
            .is_err()
    );
    assert!(service_with_owners(
        &temp,
        Arc::new(move || Some(now)),
        Arc::new(FloorOwner(
            created.record_view.owner_user_id,
            created.record_view.created_at_ns
        ))
    )
    .await
    .expect("floor service")
    .verify(created.token.as_bytes(), Endpoint::Rest, now)
    .is_err());
}

struct UnknownOwner;
impl OwnerFloors for UnknownOwner {
    fn floor_if_present(&self, _id: &[u8; 16]) -> Option<u64> {
        None
    }
}

struct FloorOwner([u8; 16], u64);
impl OwnerFloors for FloorOwner {
    fn floor_if_present(&self, id: &[u8; 16]) -> Option<u64> {
        (*id == self.0).then_some(self.1)
    }
}

#[tokio::test]
async fn read_only_node_rejects_create_and_revoke_mutations() {
    let temp = TempDir::new().expect("tempdir");
    let now = 60_000_000_000_u64;
    let config = crate::config::TokensConfig::default();
    let service = TokenService::open_with_clock(TokenServiceOptions {
        data_dir: temp.path().to_path_buf(),
        token_key: Zeroizing::new([5; 32]),
        owners: Arc::new(Owners),
        sessions: Arc::new(TokenSessionStore::new()),
        audit: audit(),
        config,
        read_only_node: true,
        clock: Arc::new(move || Some(now)),
    })
    .await
    .expect("service");
    assert!(matches!(
        service.create(new_token("ro".into(), None)).await,
        Err(TokenCreateError::ReadOnlyNode)
    ));
    assert!(matches!(
        service.revoke(TokenId([9; 16]), now).await,
        Err(TokenCreateError::ReadOnlyNode)
    ));
    assert!(matches!(
        service.revoke_user([1; 16], now).await,
        Err(TokenCreateError::ReadOnlyNode)
    ));
}

use crate::security::tokens::{
    NewToken, SessionCanceller, TokenAuditContext, TokenCreateError, TokenService,
    TokenServiceOptions,
};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::token::{
    parse_token, Endpoint, OwnerFloors, TokenChannels, TokenId, TokenMode, TokenRecord,
};
use shamir_connect::server::TokenSessionStore;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use tempfile::TempDir;
use zeroize::Zeroizing;

pub(super) fn record(id: u8) -> TokenRecord {
    TokenRecord {
        id: [id; 16],
        owner_user_id: [1; 16],
        label: format!("seed-{id}"),
        mac: [3; 32],
        key_id: 1,
        format_version: 1,
        created_at_ns: 1,
        expires_at_ns: 2,
        mode: TokenMode::ReadOnly,
        channels: TokenChannels::Both,
        databases: None,
        roles: Vec::new(),
        revoked_at_ns: None,
    }
}

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
struct CapturingAppender {
    entries: std::sync::Mutex<Vec<AuditEntry>>,
}
impl AuditAppender for CapturingAppender {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.entries
            .lock()
            .expect("appender lock")
            .push(entry.clone());
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
pub(super) fn new_token(label: String, ttl_secs: Option<u64>) -> NewToken {
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

pub(super) async fn service(
    temp: &TempDir,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
) -> TokenService {
    service_with_owners(temp, clock, Arc::new(Owners))
        .await
        .expect("open service")
}

#[tokio::test]
async fn durable_write_failures_preserve_create_revoke_and_batch_state() {
    let temp = TempDir::new().expect("tempdir");
    let now = 90_000_000_000_u64;
    let service = service(&temp, Arc::new(move || Some(now))).await;
    let counters = Arc::clone(service.counters());
    service.store_for_tests().fail_next_durable_write();
    assert!(matches!(
        service.create(new_token("reusable".into(), Some(60))).await,
        Err(TokenCreateError::Store(
            crate::security::tokens::TokenStoreError::InjectedWriteFailure
        ))
    ));
    assert_eq!(service.mirror_entry_count_hint(), 0);
    assert!(service.list([1; 16]).await.expect("empty list").is_empty());
    assert_eq!(counters.created(), 0);
    assert_eq!(counters.revoked(), 0);
    let created = service
        .create(new_token("reusable".into(), Some(60)))
        .await
        .expect("label and slot reusable");
    assert_eq!(counters.created(), 1);
    let id = TokenId(created.record_view.id);
    let canceller = Arc::new(CancelObserver {
        count: AtomicUsize::new(0),
        mirror: Arc::clone(service.mirror()),
        store: service.store_for_tests().clone(),
    });
    let service = service.with_canceller(canceller.clone());
    service.store_for_tests().fail_next_durable_write();
    assert!(matches!(
        service.revoke(id, now + 1).await,
        Err(TokenCreateError::Store(
            crate::security::tokens::TokenStoreError::InjectedWriteFailure
        ))
    ));
    assert!(service
        .mirror()
        .get(&id)
        .is_some_and(|state| !state.is_revoked()));
    assert_eq!(
        service
            .store_for_tests()
            .get_blocking(&id)
            .expect("stored")
            .unwrap()
            .revoked_at_ns,
        None
    );
    assert_eq!(counters.revoked(), 0);
    assert_eq!(canceller.count.load(Ordering::SeqCst), 0);

    let limited = TokenService::open_with_clock(TokenServiceOptions {
        data_dir: temp.path().join("limited"),
        token_key: Zeroizing::new([5; 32]),
        owners: Arc::new(Owners),
        sessions: Arc::new(TokenSessionStore::new()),
        audit: audit(),
        config: crate::config::TokensConfig {
            max_per_user: 1,
            ..Default::default()
        },
        read_only_node: false,
        clock: Arc::new(move || Some(now)),
    })
    .await
    .expect("limited service");
    limited.store_for_tests().fail_next_durable_write();
    assert!(matches!(
        limited.create(new_token("slot".into(), Some(60))).await,
        Err(TokenCreateError::Store(
            crate::security::tokens::TokenStoreError::InjectedWriteFailure
        ))
    ));
    assert!(limited
        .create(new_token("slot".into(), Some(60)))
        .await
        .is_ok());
    let second = service
        .create(new_token("second".into(), Some(60)))
        .await
        .expect("create second");
    let second_id = TokenId(second.record_view.id);
    service.store_for_tests().fail_next_durable_write();
    assert!(matches!(
        service.revoke_user([1; 16], now + 2).await,
        Err(TokenCreateError::Store(
            crate::security::tokens::TokenStoreError::InjectedWriteFailure
        ))
    ));
    assert!(service
        .mirror()
        .get(&id)
        .is_some_and(|state| !state.is_revoked()));
    assert!(service
        .mirror()
        .get(&second_id)
        .is_some_and(|state| !state.is_revoked()));
    assert_eq!(counters.revoked(), 0);
    assert_eq!(canceller.count.load(Ordering::SeqCst), 0);
    let rows = service.list([1; 16]).await.expect("owner records");
    assert!(rows.iter().all(|row| row.revoked_at_ns.is_none()));
}

#[tokio::test]
async fn token_audit_events_include_ids_without_secret_or_mac() {
    let temp = TempDir::new().expect("tempdir");
    let now = 91_000_000_000_u64;
    let appender = Arc::new(CapturingAppender {
        entries: std::sync::Mutex::new(Vec::new()),
    });
    let writer = Arc::new(AuditChainWriter::new(
        AuditChain::new([8; 32]),
        appender.clone(),
    ));
    let chain = writer.chain();
    let service = TokenService::open_with_clock(TokenServiceOptions {
        data_dir: temp.path().to_path_buf(),
        token_key: Zeroizing::new([5; 32]),
        owners: Arc::new(Owners),
        sessions: Arc::new(TokenSessionStore::new()),
        audit: writer,
        config: crate::config::TokensConfig::default(),
        read_only_node: false,
        clock: Arc::new(move || Some(now)),
    })
    .await
    .expect("service");
    let audit_context = TokenAuditContext {
        actor_username: "operator".into(),
        transport: "test-cli".into(),
        session_id_prefix: [0; 8],
    };
    let first = service
        .create_with_context(new_token("audit".into(), Some(60)), audit_context.clone())
        .await
        .expect("create");
    let first_token = first.token.as_bytes().to_vec();
    let first_parsed = parse_token(first.token.as_bytes()).expect("parse first token");
    let first_id = first.record_view.id;
    let first_mac = service
        .mirror()
        .get(&TokenId(first_id))
        .expect("mirror")
        .record()
        .mac;
    service
        .revoke_with_context(TokenId(first_id), now + 1, audit_context.clone())
        .await
        .expect("revoke");
    let second = service
        .create_with_context(
            new_token("audit-user".into(), Some(60)),
            audit_context.clone(),
        )
        .await
        .expect("create second");
    let second_token = second.token.as_bytes().to_vec();
    let second_parsed = parse_token(second.token.as_bytes()).expect("parse second token");
    let second_id = second.record_view.id;
    let second_mac = service
        .mirror()
        .get(&TokenId(second_id))
        .expect("mirror")
        .record()
        .mac;
    service
        .revoke_user_with_context([1; 16], now + 2, audit_context)
        .await
        .expect("revoke user");

    let entries = chain.snapshot();
    assert_eq!(appender.entries.lock().expect("appender entries").len(), 4);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.event.as_str())
            .collect::<Vec<_>>(),
        vec![
            "api_token_created",
            "api_token_revoked",
            "api_token_created",
            "api_token_user_revoked"
        ]
    );
    let first_parsed_id = first_parsed.id.0;
    let second_parsed_id = second_parsed.id.0;
    assert_eq!(first_parsed_id, first_id);
    assert_eq!(second_parsed_id, second_id);
    let created_details: ([u8; 16], [u8; 16]) =
        rmp_serde::from_slice(&entries[0].details_canonical_msgpack).expect("created details");
    assert_eq!(created_details, (first_id, [1; 16]));
    let second_created_details: ([u8; 16], [u8; 16]) =
        rmp_serde::from_slice(&entries[2].details_canonical_msgpack)
            .expect("second created details");
    assert_eq!(second_created_details, (second_id, [1; 16]));
    let revoked_details: ([u8; 16], [u8; 16]) =
        rmp_serde::from_slice(&entries[1].details_canonical_msgpack).expect("revoked details");
    assert_eq!(revoked_details, (first_id, [1; 16]));
    let user_details: ([u8; 16], Vec<[u8; 16]>) =
        rmp_serde::from_slice(&entries[3].details_canonical_msgpack).expect("user-revoked details");
    assert_eq!(user_details.0, [1; 16]);
    assert_eq!(user_details.1, vec![second_id]);
    let appended = appender.entries.lock().expect("appender entries");
    assert_eq!(appended.len(), 4);
    for entry in entries {
        assert_eq!(entry.user, "operator");
        assert_eq!(entry.transport, "test-cli");
        assert!(entry.ip_subnet.is_empty());
        assert_eq!(entry.session_id_prefix, [0; 8]);
        for sensitive in [
            first_token.as_slice(),
            second_token.as_slice(),
            first_parsed.secret.as_bytes().as_slice(),
            second_parsed.secret.as_bytes().as_slice(),
            first_mac.as_slice(),
            second_mac.as_slice(),
        ] {
            assert!(!entry
                .details_canonical_msgpack
                .windows(sensitive.len())
                .any(|w| w == sensitive));
        }
    }
    for (entry, appended_entry) in chain.snapshot().iter().zip(appended.iter()) {
        assert_eq!(entry.seq, appended_entry.seq);
        assert_eq!(entry.event, appended_entry.event);
        assert_eq!(
            entry.details_canonical_msgpack,
            appended_entry.details_canonical_msgpack
        );
    }
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
async fn revoke_user_skips_expired_and_already_revoked_records() {
    let temp = TempDir::new().expect("tempdir");
    let now = 71_000_000_000_u64;
    let service = service(&temp, Arc::new(move || Some(now))).await;
    let mut live = record(11);
    live.expires_at_ns = now + 10;
    let mut expired = record(12);
    expired.expires_at_ns = now - 1;
    let mut revoked = record(13);
    revoked.revoked_at_ns = Some(now - 2);
    let mut already_revoked_live = record(14);
    already_revoked_live.expires_at_ns = now + 5;
    already_revoked_live.revoked_at_ns = Some(now - 3);
    for row in [
        live.clone(),
        expired.clone(),
        revoked.clone(),
        already_revoked_live.clone(),
    ] {
        service
            .store_for_tests()
            .put(row)
            .await
            .expect("seed user row");
    }
    service.mirror().insert(revoked.clone());
    service.mirror().insert(already_revoked_live.clone());
    assert_eq!(
        service
            .revoke_user([1; 16], now)
            .await
            .expect("revoke user active rows"),
        1
    );
    assert_eq!(service.counters().revoked(), 1);
    let expired_after = service
        .store_for_tests()
        .get(TokenId(expired.id))
        .await
        .expect("expired retained")
        .unwrap();
    assert_eq!(expired_after.revoked_at_ns, None);
    assert_eq!(
        service
            .store_for_tests()
            .get(TokenId(live.id))
            .await
            .expect("active changed")
            .unwrap()
            .revoked_at_ns,
        Some(now)
    );
    assert_eq!(
        service
            .store_for_tests()
            .get(TokenId(revoked.id))
            .await
            .expect("revoked")
            .unwrap()
            .revoked_at_ns,
        revoked.revoked_at_ns
    );
    assert!(
        service
            .store_for_tests()
            .get(TokenId(already_revoked_live.id))
            .await
            .expect("already revoked")
            .unwrap()
            .revoked_at_ns
            == already_revoked_live.revoked_at_ns
    );
}

#[tokio::test]
async fn injected_id_collision_retries_and_mirror_conflict_rolls_back_put() {
    let temp = TempDir::new().expect("tempdir");
    let now = 72_000_000_000_u64;
    let mut sut = service(&temp, Arc::new(move || Some(now))).await;
    let existing = record(77);
    sut.store_for_tests()
        .put(existing.clone())
        .await
        .expect("preseed collision");
    let generated_id = [78; 16];
    let candidates = Arc::new(vec![existing.id, generated_id]);
    let next_candidate = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let source = {
        let candidates = Arc::clone(&candidates);
        let next_candidate = Arc::clone(&next_candidate);
        Arc::new(move || {
            let index = next_candidate.fetch_add(1, Ordering::SeqCst);
            let (_, secret) = shamir_connect::server::token::generate_token_parts()?;
            Ok((TokenId(candidates[index]), secret))
        })
    };
    sut.inject_generator(source);
    let created = sut
        .create(new_token("retry".into(), Some(60)))
        .await
        .expect("retry collision");
    assert_eq!(created.record_view.id, generated_id);
    assert_eq!(
        sut.store_for_tests()
            .get(TokenId(existing.id))
            .await
            .expect("preserved preseed"),
        Some(existing)
    );
    assert_eq!(next_candidate.load(Ordering::SeqCst), 2);

    let rollback_temp = TempDir::new().expect("rollback tempdir");
    let mut rollback = service(&rollback_temp, Arc::new(move || Some(now))).await;
    let rollback_id = [99; 16];
    rollback.inject_generator(Arc::new(move || {
        let (_, secret) = shamir_connect::server::token::generate_token_parts()?;
        Ok((TokenId(rollback_id), secret))
    }));
    rollback.force_mirror_insert_conflict();
    assert!(matches!(
        rollback
            .create(new_token("rollback".into(), Some(60)))
            .await,
        Err(TokenCreateError::IdCollision)
    ));
    assert_eq!(rollback.mirror_entry_count_hint(), 1);
    assert_eq!(
        rollback
            .mirror()
            .get(&TokenId(rollback_id))
            .expect("conflicting mirror entry")
            .record()
            .label,
        "rollback-conflict"
    );
    assert!(rollback
        .store_for_tests()
        .get(TokenId(rollback_id))
        .await
        .expect("rolled-back durable row")
        .is_none());
    assert_eq!(rollback.counters().created(), 0);
}

#[tokio::test]
async fn retained_non_live_record_is_revocable() {
    let temp = TempDir::new().expect("tempdir");
    let now = 70_000_000_000_u64;
    let writer = audit();
    let audit_chain = writer.chain();
    let service = TokenService::open_with_clock(TokenServiceOptions {
        data_dir: temp.path().to_path_buf(),
        token_key: Zeroizing::new([5; 32]),
        owners: Arc::new(Owners),
        sessions: Arc::new(TokenSessionStore::new()),
        audit: writer,
        config: crate::config::TokensConfig::default(),
        read_only_node: false,
        clock: Arc::new(move || Some(now)),
    })
    .await
    .expect("service");
    let created = service
        .create(new_token("retained".into(), Some(1)))
        .await
        .expect("create");
    let id = TokenId(created.record_view.id);
    service.mirror().remove(&id);
    assert_eq!(
        service
            .store_for_tests()
            .get_blocking(&id)
            .expect("durable")
            .unwrap()
            .revoked_at_ns,
        None
    );
    service.store_for_tests().fail_next_durable_write();
    assert!(matches!(
        service.revoke(id, now + 1).await,
        Err(TokenCreateError::Store(
            crate::security::tokens::TokenStoreError::InjectedWriteFailure
        ))
    ));
    assert!(service.mirror().get(&id).is_none());
    assert_eq!(service.counters().revoked(), 0);
    assert!(audit_chain
        .snapshot()
        .iter()
        .all(|entry| entry.event != "api_token_revoked"));
    assert_eq!(
        service
            .store_for_tests()
            .get_blocking(&id)
            .expect("failed retained write")
            .unwrap()
            .revoked_at_ns,
        None
    );
    assert!(service
        .revoke(id, now + 2_000_000_000)
        .await
        .expect("retained revoke"));
    assert_eq!(service.counters().revoked(), 1);

    assert_eq!(
        audit_chain
            .snapshot()
            .iter()
            .filter(|entry| entry.event == "api_token_revoked")
            .count(),
        1
    );

    let stored = service
        .store_for_tests()
        .get_blocking(&id)
        .expect("durable row")
        .unwrap();
    assert_eq!(stored.revoked_at_ns, Some(now + 2_000_000_000));

    assert_eq!(
        audit_chain
            .snapshot()
            .iter()
            .filter(|entry| entry.event == "api_token_revoked")
            .count(),
        1
    );
    let before_second_revoke_events = audit_chain
        .snapshot()
        .iter()
        .filter(|entry| entry.event == "api_token_revoked")
        .count();
    let before_second_revoke_counter = service.counters().revoked();
    assert!(!service
        .revoke(id, now + 3)
        .await
        .expect("second retained revoke"));
    assert_eq!(service.counters().revoked(), before_second_revoke_counter);
    assert_eq!(
        audit_chain
            .snapshot()
            .iter()
            .filter(|entry| entry.event == "api_token_revoked")
            .count(),
        before_second_revoke_events
    );
    assert_eq!(
        service
            .store_for_tests()
            .get_blocking(&id)
            .expect("still durable")
            .unwrap()
            .revoked_at_ns,
        stored.revoked_at_ns
    );
    assert_eq!(
        audit_chain
            .snapshot()
            .iter()
            .filter(|entry| entry.event == "api_token_revoked")
            .count(),
        1
    );
    let audit_entries = audit_chain.snapshot();
    assert!(audit_entries.iter().all(|entry| {
        entry.transport == "token"
            && entry.user.is_empty()
            && entry.ip_subnet.is_empty()
            && entry.session_id_prefix == [0; 8]
    }));
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
    assert!(!service
        .revoke(id, now + 9)
        .await
        .expect("idempotent revoke"));
}

#[tokio::test]
async fn labels_and_limits_count_only_active_unexpired_records() {
    let temp = TempDir::new().expect("tempdir");
    let now = 1_000_000_000_u64;
    let config = crate::config::TokensConfig {
        max_per_user: 1,
        ..Default::default()
    };
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
    let config = crate::config::TokensConfig {
        max_per_user: 3,
        ..Default::default()
    };
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
    let writable = service(&temp, Arc::new(move || Some(now))).await;
    let created = writable
        .create(new_token("readonly-source".into(), Some(60)))
        .await
        .expect("writable create");
    let bearer = created.token.as_bytes().to_vec();
    drop(writable);
    let service = TokenService::open_with_clock(TokenServiceOptions {
        data_dir: temp.path().to_path_buf(),
        token_key: Zeroizing::new([5; 32]),
        owners: Arc::new(Owners),
        sessions: Arc::new(TokenSessionStore::new()),
        audit: audit(),
        config: crate::config::TokensConfig::default(),
        read_only_node: true,
        clock: Arc::new(move || Some(now)),
    })
    .await
    .expect("service");
    assert!(service.verify(&bearer, Endpoint::Rest, now).is_ok());
    assert!(matches!(
        service.create(new_token("ro".into(), None)).await,
        Err(TokenCreateError::ReadOnlyNode)
    ));
    assert!(matches!(
        service.revoke(TokenId(created.record_view.id), now).await,
        Err(TokenCreateError::ReadOnlyNode)
    ));
    assert!(matches!(
        service.revoke_user([1; 16], now).await,
        Err(TokenCreateError::ReadOnlyNode)
    ));
}

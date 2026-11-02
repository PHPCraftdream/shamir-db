use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::lockout::Subnet;
use shamir_connect::server::token::{OwnerFloors, TokenChannels, TokenMode};
use shamir_connect::server::TokenSessionStore;
use tempfile::TempDir;
use zeroize::Zeroizing;

use super::super::gate::{RestGate, RestGateRails};
use crate::config::TokensConfig;
use crate::security::tokens::{NewToken, TokenService, TokenServiceOptions};
use shamir_transport_http::{AuthHeader, Endpoint, GateDecision, PeerInfo, TokenGate};

const NOW: u64 = 20_000_000_000;

struct Owners {
    floor: AtomicU64,
    present: AtomicBool,
}
impl OwnerFloors for Owners {
    fn floor_if_present(&self, id: &[u8; 16]) -> Option<u64> {
        (*id == [7; 16] && self.present.load(Ordering::SeqCst))
            .then(|| self.floor.load(Ordering::SeqCst))
    }
}
struct Audit {
    entries: Mutex<Vec<AuditEntry>>,
    appended: Notify,
    appended_count: AtomicU64,
    recording_threadids: Mutex<Vec<std::thread::ThreadId>>,
}
impl AuditAppender for Audit {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.entries
            .lock()
            .expect("audit entries")
            .push(entry.clone());
        self.recording_threadids
            .lock()
            .expect("recording thread IDs")
            .push(std::thread::current().id());
        self.appended_count.fetch_add(1, Ordering::SeqCst);
        self.appended.notify_one();
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}
struct Fixture {
    service: Arc<TokenService>,
    owners: Arc<Owners>,
    clock: Arc<AtomicU64>,
    gate: RestGate,
    audit: Arc<Audit>,
    next_label: AtomicU64,
    _temp: TempDir,
}
async fn fixture() -> Fixture {
    let temp = TempDir::new().expect("tempdir");
    let owners = Arc::new(Owners {
        floor: AtomicU64::new(0),
        present: AtomicBool::new(true),
    });
    let audit = Arc::new(Audit {
        entries: Mutex::new(Vec::new()),
        appended: Notify::new(),
        appended_count: AtomicU64::new(0),
        recording_threadids: Mutex::new(Vec::new()),
    });
    let clock = Arc::new(AtomicU64::new(NOW));
    let audit_writer = Arc::new(AuditChainWriter::new(
        AuditChain::new([2; 32]),
        audit.clone(),
    ));
    let service = Arc::new(
        TokenService::open(TokenServiceOptions {
            data_dir: temp.path().to_path_buf(),
            token_key: Zeroizing::new([3; 32]),
            owners: owners.clone(),
            sessions: Arc::new(TokenSessionStore::new()),
            audit: audit_writer,
            config: TokensConfig::default(),
            read_only_node: false,
            clock: Arc::new(|| Some(NOW)),
        })
        .await
        .expect("service"),
    );
    let gate_clock = clock.clone();
    let gate = RestGate::new(
        service.clone(),
        Arc::new(move || Some(gate_clock.load(Ordering::SeqCst))),
        RestGateRails {
            plaintext_remote: false,
            allow_plaintext_write: false,
        },
    );
    Fixture {
        service,
        owners,
        clock,
        gate,
        audit,
        next_label: AtomicU64::new(0),
        _temp: temp,
    }
}
async fn token(f: &Fixture, mode: TokenMode, channels: TokenChannels) -> String {
    f.service
        .create(NewToken {
            owner_user_id: [7; 16],
            owner_roles: vec!["reader".into()],
            label: format!("gate-test-{}", f.next_label.fetch_add(1, Ordering::SeqCst)),
            ttl_secs: Some(60),
            mode,
            channels,
            databases: None,
        })
        .await
        .expect("create")
        .token
        .as_str()
        .to_owned()
}
fn peer_for(last: u8) -> PeerInfo {
    PeerInfo {
        addr: SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(10, 0, last.saturating_add(1), 1)),
            7340,
        ),
        tls: false,
        forwarded_for: Vec::new(),
    }
}
async fn authenticate(
    gate: &RestGate,
    header: AuthHeader<'_>,
    peer: PeerInfo,
) -> GateDecision<shamir_connect::server::token::Verified> {
    gate.authenticate(header, Endpoint::Rest, peer).await
}
async fn shut(f: &Fixture) {
    f.gate.shutdown().await;
}

#[tokio::test]
async fn gate_accepts_valid_read_only_and_read_write_tokens() {
    let f = fixture().await;
    let ro = token(&f, TokenMode::ReadOnly, TokenChannels::Rest).await;
    let rw = token(&f, TokenMode::ReadWrite, TokenChannels::Rest).await;
    for value in [&ro, &rw] {
        let header = format!("Bearer {value}");
        assert!(matches!(
            authenticate(&f.gate, AuthHeader::Single(header.as_bytes()), peer_for(0)).await,
            GateDecision::Allow(_)
        ));
    }
    shut(&f).await;
}

#[tokio::test]
async fn credential_failure_classes_are_uniformly_unauthorized() {
    let f = fixture().await;
    let valid = token(&f, TokenMode::ReadOnly, TokenChannels::Rest).await;
    let mut wrong = valid.as_bytes().to_vec();
    let pos = 28;
    wrong[pos] = if wrong[pos] == b'a' { b'b' } else { b'a' };
    let wrong = format!("Bearer {}", String::from_utf8(wrong).expect("ascii token"));
    let revoked = token(&f, TokenMode::ReadOnly, TokenChannels::Rest).await;
    let parsed = shamir_connect::server::token::parse_token(revoked.as_bytes()).expect("token id");
    f.service.revoke(parsed.id, NOW + 1).await.expect("revoke");
    let expired = token(&f, TokenMode::ReadOnly, TokenChannels::Rest).await;
    let wrong_result =
        authenticate(&f.gate, AuthHeader::Single(wrong.as_bytes()), peer_for(0)).await;
    let revoked_result = authenticate(
        &f.gate,
        AuthHeader::Single(format!("Bearer {revoked}").as_bytes()),
        peer_for(1),
    )
    .await;
    f.clock.store(NOW + 61_000_000_000, Ordering::SeqCst);
    let expired_result = authenticate(
        &f.gate,
        AuthHeader::Single(format!("Bearer {expired}").as_bytes()),
        peer_for(2),
    )
    .await;
    assert!(matches!(wrong_result, GateDecision::Unauthorized));
    assert!(matches!(revoked_result, GateDecision::Unauthorized));
    assert!(matches!(expired_result, GateDecision::Unauthorized));
    shut(&f).await;
}

#[tokio::test]
async fn owner_floor_is_strict_and_missing_owner_invalidates_token() {
    let f = fixture().await;
    let value = token(&f, TokenMode::ReadOnly, TokenChannels::Rest).await;
    let header = format!("Bearer {value}");
    f.owners.floor.store(NOW, Ordering::SeqCst);
    assert!(matches!(
        authenticate(&f.gate, AuthHeader::Single(header.as_bytes()), peer_for(0)).await,
        GateDecision::Unauthorized
    ));
    f.owners.floor.store(0, Ordering::SeqCst);
    f.owners.present.store(false, Ordering::SeqCst);
    assert!(matches!(
        authenticate(&f.gate, AuthHeader::Single(header.as_bytes()), peer_for(1)).await,
        GateDecision::Unauthorized
    ));
    shut(&f).await;
}

#[tokio::test]
async fn wrong_channel_and_plaintext_write_are_forbidden() {
    let f = fixture().await;
    let ws = token(&f, TokenMode::ReadWrite, TokenChannels::Ws).await;
    assert!(
        matches!(authenticate(&f.gate, AuthHeader::Single(format!("Bearer {ws}").as_bytes()), peer_for(0)).await, GateDecision::Forbidden(code) if code == "token_channel_denied")
    );
    let value = token(&f, TokenMode::ReadWrite, TokenChannels::Rest).await;
    let gate = RestGate::new(
        f.service.clone(),
        Arc::new(|| Some(NOW)),
        RestGateRails {
            plaintext_remote: true,
            allow_plaintext_write: false,
        },
    );
    assert!(
        matches!(authenticate(&gate, AuthHeader::Single(format!("Bearer {value}").as_bytes()), peer_for(1)).await, GateDecision::Forbidden(code) if code == "token_plaintext_write_denied")
    );
    gate.shutdown().await;
    shut(&f).await;
}

#[tokio::test]
async fn malformed_unknown_multiple_and_missing_credentials_are_unauthorized() {
    let f = fixture().await;
    let unknown = b"Bearer shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    for (index, header) in [
        AuthHeader::Missing,
        AuthHeader::Multiple,
        AuthHeader::Single(b"Bearer malformed"),
        AuthHeader::Single(unknown),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(matches!(
            authenticate(&f.gate, header, peer_for(index as u8)).await,
            GateDecision::Unauthorized
        ));
    }
    shut(&f).await;
}

#[tokio::test]
async fn invalid_auth_is_limited_but_valid_same_subnet_remains_allowed() {
    let f = fixture().await;
    let valid = token(&f, TokenMode::ReadOnly, TokenChannels::Rest).await;
    let _ = authenticate(&f.gate, AuthHeader::Single(b"Bearer invalid"), peer_for(0)).await;
    let _ = authenticate(&f.gate, AuthHeader::Single(b"Bearer invalid"), peer_for(0)).await;
    let burst = authenticate(&f.gate, AuthHeader::Single(b"Bearer invalid"), peer_for(0)).await;
    assert!(matches!(burst, GateDecision::RateLimited { .. }));
    assert!(matches!(
        authenticate(
            &f.gate,
            AuthHeader::Single(format!("Bearer {valid}").as_bytes()),
            peer_for(0)
        )
        .await,
        GateDecision::Allow(_)
    ));
    shut(&f).await;
}

#[tokio::test(start_paused = true)]
async fn rate_limited_failures_are_not_audited_and_admitted_failures_are_aggregated() {
    let f = fixture().await;
    for _ in 0..2 {
        assert!(matches!(
            authenticate(
                &f.gate,
                AuthHeader::Single(b"Bearer malformed-secret"),
                peer_for(0)
            )
            .await,
            GateDecision::Unauthorized
        ));
    }
    for _ in 0..8 {
        assert!(matches!(
            authenticate(
                &f.gate,
                AuthHeader::Single(b"Bearer client-secret"),
                peer_for(0)
            )
            .await,
            GateDecision::RateLimited { .. }
        ));
    }

    let expected = f.audit.appended.notified();
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    expected.await;
    let entries = f.audit.entries.lock().expect("audit entries").clone();
    let failures: Vec<_> = entries
        .iter()
        .filter(|entry| entry.event == "api_token_auth_failures")
        .collect();
    assert_eq!(failures.len(), 1);
    let details = rmp_serde::from_slice::<(
        String,
        String,
        u64,
        usize,
        [Option<Subnet>; 4],
        u64,
        u64,
        u64,
    )>(&failures[0].details_canonical_msgpack)
    .expect("failure details_canonical_msgpack");
    assert_eq!(
        details,
        (
            "rest".into(),
            "invalid".into(),
            2,
            1,
            [Some(Subnet::V4([10, 0, 1])), None, None, None],
            0,
            0,
            0
        )
    );
    assert!(!String::from_utf8_lossy(&failures[0].details_canonical_msgpack).contains("secret"));
    assert!(!String::from_utf8_lossy(&failures[0].details_canonical_msgpack).contains("client"));
    {
        let recording_threads = f
            .audit
            .recording_threadids
            .lock()
            .expect("recording thread IDs");
        assert!(!recording_threads.is_empty());
        assert!(recording_threads
            .iter()
            .all(|thread_id| *thread_id != std::thread::current().id()));
    }
    let count = f.audit.appended_count.load(Ordering::SeqCst);

    for _ in 0..3 {
        assert!(matches!(
            authenticate(
                &f.gate,
                AuthHeader::Single(b"Bearer more-client-secret"),
                peer_for(0)
            )
            .await,
            GateDecision::RateLimited { .. }
        ));
    }
    shut(&f).await;
    assert_eq!(f.audit.appended_count.load(Ordering::SeqCst), count);
}
#[tokio::test]
async fn failure_fixture_records_audit_entries_and_missing_header_does_not_hit_failure_path() {
    let f = fixture().await;
    for _ in 0..100 {
        assert!(matches!(
            authenticate(&f.gate, AuthHeader::Missing, peer_for(0)).await,
            GateDecision::Unauthorized
        ));
    }
    assert!(matches!(
        authenticate(
            &f.gate,
            AuthHeader::Single(b"Bearer malformed"),
            peer_for(0)
        )
        .await,
        GateDecision::Unauthorized
    ));
    assert!(matches!(
        authenticate(
            &f.gate,
            AuthHeader::Single(b"Bearer malformed"),
            peer_for(0)
        )
        .await,
        GateDecision::Unauthorized
    ));
    assert!(matches!(
        authenticate(
            &f.gate,
            AuthHeader::Single(b"Bearer malformed"),
            peer_for(0)
        )
        .await,
        GateDecision::RateLimited { .. }
    ));
    shut(&f).await;
}

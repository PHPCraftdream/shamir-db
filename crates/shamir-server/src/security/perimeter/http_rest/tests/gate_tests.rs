use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::lockout::subnet_of;
use shamir_connect::server::rate_limit::{InMemoryRateLimiter, RateDecision, RateLimiter};
use shamir_connect::server::token::{OwnerFloors, TokenChannels, TokenMode};
use shamir_connect::server::TokenSessionStore;
use tempfile::TempDir;
use zeroize::Zeroizing;

use super::super::gate::{RestGate, RestGateRails};
use crate::config::TokensConfig;
use crate::security::tokens::{NewToken, TokenService, TokenServiceOptions};
use shamir_transport_http::{AuthHeader, Endpoint, GateDecision, PeerInfo, TokenGate};

struct Owners;
impl OwnerFloors for Owners {
    fn floor_if_present(&self, id: &[u8; 16]) -> Option<u64> {
        (*id == [7; 16]).then_some(0)
    }
}
struct Audit;
impl AuditAppender for Audit {
    fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}
fn audit() -> Arc<AuditChainWriter> {
    Arc::new(AuditChainWriter::new(
        AuditChain::new([2; 32]),
        Arc::new(Audit),
    ))
}
async fn service(temp: &TempDir, now: u64) -> TokenService {
    TokenService::open(TokenServiceOptions {
        data_dir: temp.path().to_path_buf(),
        token_key: Zeroizing::new([3; 32]),
        owners: Arc::new(Owners),
        sessions: Arc::new(TokenSessionStore::new()),
        audit: audit(),
        config: TokensConfig::default(),
        read_only_node: false,
        clock: Arc::new(move || Some(now)),
    })
    .await
    .expect("service")
}
fn peer_for(last: u8) -> PeerInfo {
    PeerInfo {
        addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, last + 1, 1)), 7340),
        tls: false,
        forwarded_for: Vec::new(),
    }
}

fn peer() -> PeerInfo {
    peer_for(0)
}

#[tokio::test]
async fn token_gate_returns_uniform_unauthorized_for_all_credential_failures() {
    let temp = TempDir::new().expect("tempdir");
    let service = Arc::new(service(&temp, 10_000_000_000).await);
    let gate = RestGate::new(
        service,
        Arc::new(|| Some(10_000_000_000)),
        RestGateRails {
            plaintext_remote: false,
            allow_plaintext_write: false,
        },
    );
    for (index, header) in [
        AuthHeader::Missing,
        AuthHeader::Multiple,
        AuthHeader::Single(b"Bearer malformed"),
        AuthHeader::Single(
            b"Bearer shk1.aaaaaaaaaaaaaaaaaaaaaa.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let result = gate
            .authenticate(header, Endpoint::Rest, peer_for(index as u8))
            .await;
        assert!(matches!(result, GateDecision::Unauthorized));
    }
}

#[tokio::test]
async fn read_write_token_is_forbidden_on_remote_plaintext_without_rail() {
    let temp = TempDir::new().expect("tempdir");
    let service = Arc::new(service(&temp, 20_000_000_000).await);
    let created = service
        .create(NewToken {
            owner_user_id: [7; 16],
            owner_roles: vec!["reader".into()],
            label: "rw".into(),
            ttl_secs: Some(60),
            mode: TokenMode::ReadWrite,
            channels: TokenChannels::Rest,
            databases: None,
        })
        .await
        .expect("create");
    let gate = RestGate::new(
        service,
        Arc::new(|| Some(20_000_000_000)),
        RestGateRails {
            plaintext_remote: true,
            allow_plaintext_write: false,
        },
    );
    let header = format!("Bearer {}", created.token.as_str());
    assert!(
        matches!(gate.authenticate(AuthHeader::Single(header.as_bytes()), Endpoint::Rest, peer()).await, GateDecision::Forbidden(code) if code == "token_plaintext_write_denied")
    );
}

#[test]
fn auth_failure_subnet_limiter_is_available_without_scram_lockout() {
    let limiter = InMemoryRateLimiter::new(1);
    let subnet = subnet_of(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)));
    assert_eq!(limiter.check(subnet, 1), RateDecision::Allowed);
}

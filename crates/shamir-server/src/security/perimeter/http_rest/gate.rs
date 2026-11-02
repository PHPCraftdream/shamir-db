use std::net::IpAddr;
use std::sync::Arc;

use shamir_connect::server::lockout::subnet_of;
use shamir_connect::server::rate_limit::{InMemoryRateLimiter, RateDecision, RateLimiter};
use shamir_connect::server::token::{Endpoint as TokenEndpoint, Verified, VerifyFailure};
use shamir_transport_http::{
    AuthHeader, Endpoint as HttpEndpoint, GateDecision, PeerInfo, TokenGate,
};

use crate::security::tokens::TokenService;

/// Listener policy rails needed during REST token authentication.
#[derive(Clone, Copy)]
pub struct RestGateRails {
    /// Whether listener is configured for remote plaintext.
    pub plaintext_remote: bool,
    /// Whether read-write tokens are allowed over remote plaintext.
    pub allow_plaintext_write: bool,
}

/// REST bearer gate backed by TokenService's in-memory verification mirror.
pub struct RestGate {
    service: Arc<TokenService>,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    rails: RestGateRails,
    failures: Arc<InMemoryRateLimiter>,
}

impl RestGate {
    /// Construct using shared service and a current Unix-nanosecond clock.
    pub fn new(
        service: Arc<TokenService>,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
        rails: RestGateRails,
    ) -> Self {
        let now = clock().unwrap_or_default();
        Self {
            service,
            clock,
            rails,
            failures: Arc::new(InMemoryRateLimiter::new(now)),
        }
    }

    fn subnet(ip: IpAddr) -> String {
        match subnet_of(ip) {
            shamir_connect::server::lockout::Subnet::V4(value) => format!("v4:{value:?}"),
            shamir_connect::server::lockout::Subnet::V6(value) => format!("v6:{value:?}"),
        }
    }

    fn bearer(header: AuthHeader<'_>) -> Option<&[u8]> {
        let AuthHeader::Single(bytes) = header else {
            return None;
        };
        let split = bytes.iter().position(|byte| *byte == b' ')?;
        let (scheme, value) = bytes.split_at(split);
        if !scheme.eq_ignore_ascii_case(b"Bearer")
            || value.len() < 2
            || value[0] != b' '
            || value[1] == b' '
            || value[1] == b'\t'
            || value[1] == b'\r'
            || value[1] == b'\n'
        {
            return None;
        }
        Some(&value[1..])
    }

    fn failed(&self, ip: IpAddr, now: u64) -> GateDecision<Verified> {
        let decision = self.failures.check(subnet_of(ip), now);
        self.service.audit_auth_failure(&Self::subnet(ip), now);
        metrics::counter!("shamir_token_auth_total", "channel" => "rest", "result" => "invalid")
            .increment(1);
        tracing::warn!(
            channel = "rest",
            result = "invalid",
            "token authentication failed"
        );
        match decision {
            RateDecision::Allowed => GateDecision::Unauthorized,
            RateDecision::RateLimited { retry_after_secs } => GateDecision::RateLimited {
                retry_after_secs: u64::from(retry_after_secs.max(1)),
            },
        }
    }
}

impl TokenGate for RestGate {
    type Grant = Verified;

    async fn authenticate<'a>(
        &'a self,
        auth: AuthHeader<'a>,
        _endpoint: HttpEndpoint,
        peer: PeerInfo,
    ) -> GateDecision<Self::Grant> {
        let Some(now) = (self.clock)() else {
            return GateDecision::Unavailable {
                retry_after_secs: 1,
            };
        };
        let Some(token) = Self::bearer(auth) else {
            if matches!(auth, AuthHeader::Missing) {
                return GateDecision::Unauthorized;
            }
            return self.failed(peer.addr.ip(), now);
        };
        match self.service.verify(token, TokenEndpoint::Rest, now) {
            Ok(grant) => {
                if self.rails.plaintext_remote
                    && !self.rails.allow_plaintext_write
                    && grant.state.record().mode
                        == shamir_connect::server::token::TokenMode::ReadWrite
                {
                    return GateDecision::Forbidden("token_plaintext_write_denied".into());
                }
                metrics::counter!("shamir_token_auth_total", "channel" => "rest", "result" => "success")
                    .increment(1);
                GateDecision::Allow(grant)
            }
            Err(VerifyFailure::ChannelDenied) => {
                GateDecision::Forbidden("token_channel_denied".into())
            }
            Err(
                VerifyFailure::Invalid
                | VerifyFailure::Revoked
                | VerifyFailure::Expired
                | VerifyFailure::OwnerInvalid,
            ) => self.failed(peer.addr.ip(), now),
        }
    }

    fn still_valid(&self, grant: &Self::Grant) -> bool {
        (self.clock)().is_some_and(|now| self.service.still_valid(grant, now))
    }
}

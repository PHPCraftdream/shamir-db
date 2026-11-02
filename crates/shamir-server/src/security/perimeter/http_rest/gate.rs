use std::net::IpAddr;
use std::sync::Arc;

use shamir_connect::server::lockout::subnet_of;
use shamir_connect::server::rate_limit::RateDecision;
use shamir_connect::server::token::{
    Endpoint as TokenEndpoint, TokenMode, Verified, VerifyFailure,
};
use shamir_transport_http::{
    AuthHeader, Endpoint as HttpEndpoint, GateDecision, PeerInfo, TokenGate,
};

use super::bucket::BucketCache;
use super::maintenance::RestMaintenance;
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
    maintenance: RestMaintenance,
}

impl RestGate {
    /// Stop and join the maintenance worker. If this future is cancelled while
    /// awaiting, the worker may continue until its shutdown signal is observed.
    pub async fn shutdown(&self) {
        self.maintenance.shutdown().await;
    }

    /// Construct using shared service and a current Unix-nanosecond clock.
    ///
    /// # Panics
    ///
    /// Panics unless called from within a Tokio runtime; the owned maintenance
    /// worker is spawned on the current runtime.
    pub fn new(
        service: Arc<TokenService>,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
        rails: RestGateRails,
    ) -> Self {
        Self::new_with_buckets(service, clock, rails, Arc::new(BucketCache::default()))
    }

    pub(super) fn new_with_buckets(
        service: Arc<TokenService>,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
        rails: RestGateRails,
        buckets: Arc<BucketCache>,
    ) -> Self {
        let maintenance = RestMaintenance::new(
            service.audit_writer(),
            Arc::clone(&clock),
            service.maintenance_shutdown(),
            buckets,
        );
        metrics::describe_counter!(
            "shamir_token_auth_total",
            metrics::Unit::Count,
            "REST token authentication outcomes by fixed channel and result labels"
        );
        for result in [
            "success",
            "invalid",
            "forbidden",
            "rate_limited",
            "unavailable",
        ] {
            metrics::counter!("shamir_token_auth_total", "channel" => "rest", "result" => result)
                .increment(0);
        }
        Self {
            service,
            clock,
            rails,
            maintenance,
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
        let decision = self.maintenance.failures().check(subnet_of(ip), now);
        if matches!(decision, RateDecision::Allowed) {
            self.maintenance.record_failure(subnet_of(ip), now);
            self.service
                .counters()
                .count_auth_result(TokenEndpoint::Rest, "invalid");
            tracing::trace!(
                channel = "rest",
                result = "invalid",
                "token authentication failed"
            );
        }
        match decision {
            RateDecision::Allowed => GateDecision::Unauthorized,
            RateDecision::RateLimited { retry_after_secs } => {
                self.service
                    .counters()
                    .count_auth_result(TokenEndpoint::Rest, "rate_limited");
                GateDecision::RateLimited {
                    retry_after_secs: u64::from(retry_after_secs.max(1)),
                }
            }
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
            self.service
                .counters()
                .count_auth_result(TokenEndpoint::Rest, "unavailable");
            return GateDecision::Unavailable {
                retry_after_secs: 1,
            };
        };
        let Some(token) = Self::bearer(auth) else {
            if matches!(auth, AuthHeader::Missing) {
                self.service
                    .counters()
                    .count_auth_result(TokenEndpoint::Rest, "invalid");
                return GateDecision::Unauthorized;
            }
            return self.failed(peer.addr.ip(), now);
        };
        match self.service.verify(token, TokenEndpoint::Rest, now) {
            Ok(grant) => {
                if self.rails.plaintext_remote
                    && !self.rails.allow_plaintext_write
                    && grant.state.record().mode == TokenMode::ReadWrite
                {
                    self.service
                        .counters()
                        .count_auth_result(TokenEndpoint::Rest, "forbidden");
                    self.maintenance.record_exposed(
                        grant.state.record().id,
                        grant.state.record().owner_user_id,
                        subnet_of(peer.addr.ip()),
                        now,
                    );
                    return GateDecision::Forbidden("token_plaintext_write_denied".into());
                }
                self.service
                    .counters()
                    .count_auth_result(TokenEndpoint::Rest, "success");
                GateDecision::Allow(grant)
            }
            Err(VerifyFailure::ChannelDenied) => {
                self.service
                    .counters()
                    .count_auth_result(TokenEndpoint::Rest, "forbidden");
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

    fn revalidate(&self, grant: &Self::Grant) -> GateDecision<()> {
        let Some(now) = (self.clock)() else {
            return GateDecision::Unavailable {
                retry_after_secs: 1,
            };
        };
        if self.service.still_valid(grant, now) {
            GateDecision::Allow(())
        } else {
            GateDecision::Unauthorized
        }
    }
}

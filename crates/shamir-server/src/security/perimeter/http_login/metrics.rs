//! Metrics for HTTP login use fixed labels and no identity data.

/// Fixed result vocabulary for login outcome metrics.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum LoginResult {
    /// Challenge returned.
    Challenge,
    /// Token issued.
    Issued,
    /// Authentication rejected.
    AuthenticationFailed,
    /// Request denied by policy.
    PolicyDenied,
    /// Source rate limited.
    RateLimited,
    /// Temporary server failure.
    ServerBusy,
    /// Malformed request.
    InvalidRequest,
}
impl LoginResult {
    /// Stable low-cardinality result label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Challenge => "challenge",
            Self::Issued => "issued",
            Self::AuthenticationFailed => "authentication_failed",
            Self::PolicyDenied => "policy_denied",
            Self::RateLimited => "rate_limited",
            Self::ServerBusy => "server_busy",
            Self::InvalidRequest => "invalid_request",
        }
    }
}

/// Apply an occupancy delta to the count-only pending gauge.
pub fn update_pending(delta: isize) {
    metrics::gauge!("shamir_login_http_pending").increment(delta as f64);
}
/// Record pending-entry evictions with a fixed reason vocabulary.
pub fn record_eviction(reason: &'static str) {
    let reason = match reason {
        "fair_share" => "fair_share",
        "expired" => "expired",
        _ => return,
    };
    metrics::counter!("shamir_login_http_evictions", "reason" => reason).increment(1);
}
/// Record a login outcome with fixed, low-cardinality labels.
pub fn record_result(endpoint: &'static str, result: LoginResult) {
    metrics::counter!("shamir_login_http_total", "endpoint" => endpoint, "result" => result.as_str()).increment(1);
}
/// Record bounded budget-table accounting drops with a fixed reason label.
pub fn record_budget_drop(reason: &'static str) {
    let reason = match reason {
        "arrival_table_full" => "arrival_table_full",
        "credit_table_full" => "credit_table_full",
        _ => return,
    };
    metrics::counter!("shamir_login_http_accounting_dropped", "reason" => reason).increment(1);
}
/// Record the count-only ineligible-account operator gauge.
pub fn set_ineligible_users(count: usize) {
    metrics::gauge!("shamir_login_http_kdf_ineligible_users").set(count as f64);
}
/// Observe BEGIN duration using the SDK's fixed histogram buckets.
pub fn observe_begin(seconds: f64) {
    metrics::histogram!("shamir_login_http_begin_duration_seconds").record(seconds);
}
/// Observe FINISH duration using the SDK's fixed histogram buckets.
pub fn observe_finish(seconds: f64) {
    metrics::histogram!("shamir_login_http_finish_duration_seconds").record(seconds);
}

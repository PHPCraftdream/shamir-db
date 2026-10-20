use serde::Deserialize;
use shamir_db::DbError;
use shamir_query_types::batch::VisibilityWait;

use crate::config::ConfigError;

/// Server-owned visibility policy. Zero budgets are immediate predicate checks.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TransactionsConfig {
    pub visibility_ack_mode: String,
    pub visibility_ack_timeout_ms: u64,
    pub max_visibility_wait_ms: u64,
    pub allow_strict_visibility_wait: bool,
    pub read_visibility_timeout_ms: u64,
}

impl Default for TransactionsConfig {
    fn default() -> Self {
        Self {
            visibility_ack_mode: "strict".into(),
            visibility_ack_timeout_ms: 5000,
            max_visibility_wait_ms: 30000,
            allow_strict_visibility_wait: true,
            read_visibility_timeout_ms: 5000,
        }
    }
}

impl TransactionsConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        match self.visibility_ack_mode.as_str() {
            "strict" | "bounded" => {}
            _ => {
                return Err(ConfigError::Validation(
                    "transactions.visibility_ack_mode must be strict or bounded".into(),
                ));
            }
        }
        for (name, value) in [
            ("visibility_ack_timeout_ms", self.visibility_ack_timeout_ms),
            ("max_visibility_wait_ms", self.max_visibility_wait_ms),
            (
                "read_visibility_timeout_ms",
                self.read_visibility_timeout_ms,
            ),
        ] {
            if value > u64::from(u32::MAX) {
                return Err(ConfigError::Validation(format!(
                    "transactions.{name} must be in 0..={} milliseconds",
                    u32::MAX,
                )));
            }
        }
        if self.visibility_ack_timeout_ms > self.max_visibility_wait_ms {
            return Err(ConfigError::Validation(
                "transactions.visibility_ack_timeout_ms exceeds max_visibility_wait_ms".into(),
            ));
        }
        Ok(())
    }

    /// Resolve commit policy before writes. Strict permission applies only to
    /// explicit network overrides, not the server-owned Strict default.
    pub fn resolve_commit_wait(
        &self,
        wait: Option<VisibilityWait>,
    ) -> Result<VisibilityWait, DbError> {
        match wait {
            Some(wait) => self.resolve_wait(wait),
            None if self.visibility_ack_mode == "strict" => Ok(VisibilityWait::Strict),
            None => Ok(VisibilityWait::Bounded {
                timeout_ms: self.visibility_ack_timeout_ms,
            }),
        }
    }

    /// Resolve a network override without silently weakening or clamping it.
    pub fn resolve_wait(&self, wait: VisibilityWait) -> Result<VisibilityWait, DbError> {
        wait.validate(
            self.max_visibility_wait_ms,
            self.allow_strict_visibility_wait,
        )
        .map_err(|reason| DbError::VisibilityWaitInvalid {
            reason: reason.into(),
        })?;
        Ok(wait)
    }
}

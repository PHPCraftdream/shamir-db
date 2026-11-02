/// Cheap process-local service counters.
#[derive(Debug, Default)]
pub struct TokenCounters {
    created: std::sync::atomic::AtomicU64,
    revoked: std::sync::atomic::AtomicU64,
    verified_ok: std::sync::atomic::AtomicU64,
    verified_failed: std::sync::atomic::AtomicU64,
}

impl TokenCounters {
    /// Number of token creations accepted.
    pub fn created(&self) -> u64 {
        self.created.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of first-time token revocations accepted.
    pub fn revoked(&self) -> u64 {
        self.revoked.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of successful verifications.
    pub fn verified_ok(&self) -> u64 {
        self.verified_ok.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Number of failed verifications.
    pub fn verified_failed(&self) -> u64 {
        self.verified_failed
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn count_created(&self) {
        self.created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn count_revoked(&self) {
        self.revoked
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn count_verified(&self, success: bool) {
        let counter = if success {
            &self.verified_ok
        } else {
            &self.verified_failed
        };
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

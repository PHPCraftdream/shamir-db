use std::sync::Arc;

use async_trait::async_trait;
use shamir_query_types::wire::repl::ReplResponse;

use crate::ShamirDb;

use super::SeedRefusal;

/// Injected server transport seam. Embedded homes fail closed without a probe.
/// async-trait (dyn): Send futures support TLS sources and in-process tests.
#[async_trait]
pub trait ResumeProbe: Send + Sync {
    async fn hello(&self, upstream: &str) -> Result<ReplResponse, SeedRefusal>;
}

impl ShamirDb {
    /// Install the authenticated Hello transport before accepting admin work.
    pub fn set_replication_resume_probe(&self, probe: Arc<dyn ResumeProbe>) {
        *self
            .replication_resume_probe
            .write()
            .expect("resume probe lock poisoned") = Some(probe);
    }

    pub(crate) async fn probe_seed_upstream(
        &self,
        upstream: &str,
    ) -> Result<ReplResponse, SeedRefusal> {
        // Startup-only population: clone out before I/O, no guard across await.
        let probe = {
            self.replication_resume_probe
                .read()
                .map_err(|_| SeedRefusal::UpstreamUnavailable)?
                .clone()
                .ok_or(SeedRefusal::UpstreamUnavailable)?
        };
        probe.hello(upstream).await
    }
}

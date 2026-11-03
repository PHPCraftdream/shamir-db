use crate::security::tokens::{TokenStore, TokenStoreError};
use shamir_connect::server::token::{TokenId, TokenMirror};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

/// Retention collector sharing the service's store, mirror and mutation gates.
pub(super) struct GcService {
    store: TokenStore,
    mirror: Arc<TokenMirror>,
    mutation_gates: Arc<[Arc<AsyncMutex<()>>; 64]>,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
}

impl GcService {
    pub(super) fn new(
        store: TokenStore,
        mirror: Arc<TokenMirror>,
        mutation_gates: Arc<[Arc<AsyncMutex<()>>; 64]>,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    ) -> Self {
        Self {
            store,
            mirror,
            mutation_gates,
            clock,
        }
    }

    pub(super) async fn collect(&self) -> Result<usize, TokenStoreError> {
        let Some(now) = (self.clock)() else {
            return Ok(0);
        };
        let mut gates = Vec::with_capacity(64);
        for gate in self.mutation_gates.iter() {
            gates.push(Arc::clone(gate).lock_owned().await);
        }
        let store = self.store.clone();
        let mirror = Arc::clone(&self.mirror);
        let dispatcher = tracing::dispatcher::get_default(Clone::clone);
        tokio::task::spawn_blocking(move || {
            tracing::dispatcher::with_default(&dispatcher, || {
                let _gates = gates;
                store.with_locked_records(|db, ks| {
                    let mut batch = db.batch().durability(Some(fjall::PersistMode::SyncAll));
                    let mut ids = Vec::new();
                    for row in store.scan_locked()? {
                        let expired =
                            row.expires_at_ns.saturating_add(30 * 86_400_000_000_000) < now;
                        let revoked = row
                            .revoked_at_ns
                            .is_some_and(|at| at.saturating_add(30 * 86_400_000_000_000) < now);
                        if expired || revoked {
                            let id = TokenId(row.id);
                            batch.remove(ks, id.0);
                            ids.push(id);
                        }
                    }
                    if !ids.is_empty() {
                        store.fail_locked_write()?;
                        batch.commit()?;
                        for id in &ids {
                            mirror.remove(id);
                        }
                    }
                    Ok(ids.len())
                })
            })
        })
        .await
        .map_err(|_| TokenStoreError::Join)?
    }
}

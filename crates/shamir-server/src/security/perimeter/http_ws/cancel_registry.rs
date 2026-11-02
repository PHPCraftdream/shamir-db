use std::sync::Arc;

use scc::HashMap;
use shamir_collections::THasher;
use shamir_connect::server::token::TokenId;
use tokio_util::sync::CancellationToken;

use crate::security::tokens::SessionCanceller;

type TokenEntries = HashMap<TokenId, TokenEntry, THasher>;

struct TokenEntry {
    parent: CancellationToken,
    connections: usize,
}

/// Tracks live WebSocket connections by token so revocation cancels all of them.
pub(crate) struct TokenWsCancelRegistry {
    entries: TokenEntries,
    #[cfg(test)]
    entry_count: std::sync::atomic::AtomicUsize,
}

impl Default for TokenWsCancelRegistry {
    fn default() -> Self {
        Self {
            entries: HashMap::with_hasher(THasher::default()),
            #[cfg(test)]
            entry_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl TokenWsCancelRegistry {
    /// Registers one connection and returns its connection-scoped cancellation handle.
    pub(crate) fn register(self: &Arc<Self>, id: TokenId) -> TokenWsRegistration {
        let entry = self.entries.entry_sync(id);
        let parent = match entry {
            scc::hash_map::Entry::Occupied(mut occupied) => {
                let connections = occupied.get().connections.saturating_add(1);
                let token = occupied.get().parent.clone();
                occupied.get_mut().connections = connections;
                token
            }
            scc::hash_map::Entry::Vacant(vacant) => {
                let token = CancellationToken::new();
                vacant.insert_entry(TokenEntry {
                    parent: token.clone(),
                    connections: 1,
                });
                #[cfg(test)]
                self.entry_count
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                token
            }
        };
        TokenWsRegistration {
            registry: Arc::clone(self),
            id,
            token: parent.child_token(),
        }
    }
    #[cfg(test)]
    pub(crate) fn contains_entry(&self, id: &TokenId) -> bool {
        self.entries.contains_sync(id)
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.entry_count.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl SessionCanceller for TokenWsCancelRegistry {
    fn cancel_token(&self, id: &TokenId) {
        if let Some(entry) = self.entries.get_sync(id) {
            entry.parent.cancel();
        }
    }
}

/// RAII registration for one live token WebSocket connection.
pub(crate) struct TokenWsRegistration {
    registry: Arc<TokenWsCancelRegistry>,
    id: TokenId,
    token: CancellationToken,
}

impl TokenWsRegistration {
    /// Returns the connection-specific child cancellation token.
    pub(crate) fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Drop for TokenWsRegistration {
    fn drop(&mut self) {
        match self.registry.entries.entry_sync(self.id) {
            scc::hash_map::Entry::Occupied(mut occupied) => {
                let entry = occupied.get_mut();
                entry.connections = entry.connections.saturating_sub(1);
                if entry.connections == 0 {
                    occupied.remove_entry();
                    #[cfg(test)]
                    self.registry
                        .entry_count
                        .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                }
            }
            scc::hash_map::Entry::Vacant(_) => {}
        }
    }
}

use std::hash::BuildHasher;
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use shamir_collections::{TFxSet, THasher};
use shamir_engine::function::NetGateway;

use super::curl_gateway::CurlNetGateway;

type CachedGrants = (Vec<String>, Arc<dyn NetGateway>);

pub(in crate::shamir_db) struct NetGrantsCache {
    allowlist: Vec<String>,
    cached: [ArcSwapOption<CachedGrants>; 64],
    builtin: Arc<dyn NetGateway>,
}

impl NetGrantsCache {
    pub(in crate::shamir_db) fn new(allowlist: Vec<String>) -> Self {
        let builtin = Arc::new(CurlNetGateway::new(allowlist.clone()));
        Self {
            allowlist,
            cached: std::array::from_fn(|_| ArcSwapOption::empty()),
            builtin,
        }
    }

    fn slot_index(grants: &[String]) -> usize {
        (THasher::default().hash_one(grants) as usize) % 64
    }

    pub(in crate::shamir_db) fn gateway(&self, grants: Option<&[String]>) -> Arc<dyn NetGateway> {
        let Some(grants) = grants else {
            return self.builtin.clone();
        };
        let slot = &self.cached[Self::slot_index(grants)];
        if let Some(cached) = slot.load_full() {
            if cached.0 == grants {
                return cached.1.clone();
            }
        }
        let grant_set: TFxSet<&str> = grants.iter().map(String::as_str).collect();
        let effective = self
            .allowlist
            .iter()
            .filter(|host| grant_set.contains(host.as_str()))
            .cloned()
            .collect();
        let gateway: Arc<dyn NetGateway> = Arc::new(CurlNetGateway::new(effective));
        slot.store(Some(Arc::new((grants.to_vec(), gateway.clone()))));
        gateway
    }
}

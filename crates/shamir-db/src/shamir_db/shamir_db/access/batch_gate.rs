use std::hash::BuildHasher;

use dashmap::DashMap;
use shamir_collections::THasher;
use shamir_engine::query::batch::Authorized;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath, ResourcePathRef};

use crate::access::Action;
use crate::query::batch::BatchError;
use crate::query::batch::BatchRequest;
use crate::shamir_db::shamir_db::ShamirDb;

pub(super) struct AclMemo {
    pub(super) metas: DashMap<u64, Vec<(ResourcePath, ResourceMeta)>, THasher>,
    pub(super) membership: DashMap<(u64, u64), bool, THasher>,
}

impl Default for AclMemo {
    fn default() -> Self {
        Self {
            metas: DashMap::with_hasher_and_shard_amount(THasher::default(), 2),
            membership: DashMap::with_hasher_and_shard_amount(THasher::default(), 2),
        }
    }
}

impl AclMemo {
    fn hash(path: ResourcePathRef<'_>) -> u64 {
        THasher::default().hash_one(path)
    }

    pub(super) fn meta(&self, path: ResourcePathRef<'_>) -> Option<ResourceMeta> {
        self.metas.get(&Self::hash(path)).and_then(|bucket| {
            bucket
                .iter()
                .find(|(key, _)| key.as_ref() == path)
                .map(|(_, meta)| meta.clone())
        })
    }

    pub(super) fn insert_meta(&self, path: ResourcePathRef<'_>, meta: ResourceMeta) {
        let mut bucket = self.metas.entry(Self::hash(path)).or_default();
        if let Some((_, value)) = bucket.iter_mut().find(|(key, _)| key.as_ref() == path) {
            *value = meta;
        } else {
            bucket.push((path.to_owned(), meta));
        }
    }
}

struct BatchAccessGate<'a> {
    db: &'a ShamirDb,
    memo: AclMemo,
}

#[async_trait::async_trait]
impl shamir_engine::query::batch::AccessGate for BatchAccessGate<'_> {
    async fn check(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
    ) -> Result<(), crate::access::AccessError> {
        self.db
            .authorize_access_in(actor, path, action, Some(&self.memo))
            .await
    }
}

impl ShamirDb {
    pub(crate) async fn authorize_batch<'r>(
        &self,
        request: &'r BatchRequest,
        actor: Actor,
        db_name: &'r str,
    ) -> Result<Authorized<'r>, BatchError> {
        let gate = BatchAccessGate {
            db: self,
            memo: AclMemo::default(),
        };
        Box::pin(Authorized::authorize(request, actor, db_name, &gate)).await
    }
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;

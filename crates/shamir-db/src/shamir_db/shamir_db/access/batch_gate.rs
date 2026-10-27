use dashmap::DashMap;
use shamir_collections::THasher;
use shamir_engine::query::batch::Authorized;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};

use crate::access::Action;
use crate::query::batch::BatchError;
use crate::query::batch::BatchRequest;
use crate::shamir_db::shamir_db::ShamirDb;

pub(super) struct AclMemo {
    pub(super) metas: DashMap<ResourcePath, ResourceMeta, THasher>,
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

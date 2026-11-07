use shamir_collections::TFxMap;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::access::{AccessError, Action, Actor, Mode, ResourceMeta, ResourcePath};
use crate::query::batch::AccessGate;

use super::super::ShamirDb;

#[async_trait::async_trait]
impl AccessGate for ShamirDb {
    async fn check(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
    ) -> Result<(), AccessError> {
        self.authorize_access(actor, path, action).await
    }
}

/// Build one access-tree node as a [`QueryValue`] map, resolving the
/// owner/group ids to names via the supplied lookups. Callers attach
/// `children` afterwards (leaf nodes keep the empty list; functions drop it).
pub(super) fn access_node(
    name: &str,
    kind: &str,
    meta: &ResourceMeta,
    name_of: &TFxMap<u64, String>,
    group_name_of: &TFxMap<u64, String>,
) -> QueryValue {
    let owner_id = meta.owner.to_owner_id();
    let mut m = new_map();
    m.insert("name".to_string(), QueryValue::Str(name.to_string()));
    m.insert("kind".to_string(), QueryValue::Str(kind.to_string()));
    m.insert("owner".to_string(), QueryValue::Int(owner_id as i64));
    m.insert(
        "owner_name".to_string(),
        name_of
            .get(&owner_id)
            .map(|n| QueryValue::Str(n.clone()))
            .unwrap_or(QueryValue::Null),
    );
    m.insert(
        "group".to_string(),
        meta.group
            .map(|g| QueryValue::Int(g as i64))
            .unwrap_or(QueryValue::Null),
    );
    m.insert(
        "group_name".to_string(),
        meta.group
            .and_then(|g| group_name_of.get(&g))
            .map(|n| QueryValue::Str(n.clone()))
            .unwrap_or(QueryValue::Null),
    );
    m.insert("mode".to_string(), QueryValue::Int(meta.mode as i64));
    m.insert(
        "setuid".to_string(),
        QueryValue::Bool(Mode::is_setuid(meta.mode)),
    );
    m.insert("children".to_string(), QueryValue::List(Vec::new()));
    QueryValue::Map(m)
}

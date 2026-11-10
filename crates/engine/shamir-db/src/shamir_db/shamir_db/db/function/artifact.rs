use shamir_engine::function::{FunctionMeta, Security};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::access::{Action, Actor, Mode, ResourceMeta, ResourcePath};
use crate::{DbError, DbResult};

use super::super::super::ShamirDb;

pub(super) const ARTIFACT_ID_FIELD: &str = "artifact_id";

pub(super) fn fresh_artifact_id() -> String {
    RecordId::new().to_string()
}

pub(super) fn artifact_id_from_record(record: &QueryValue) -> Result<Option<String>, String> {
    let QueryValue::Map(fields) = record else {
        return Err("function catalogue row is not a map".to_string());
    };
    match fields.get(ARTIFACT_ID_FIELD) {
        None => Ok(None),
        Some(QueryValue::Str(artifact_id)) => Ok(Some(artifact_id.clone())),
        Some(_) => Err("function catalogue artifact_id is not a string".to_string()),
    }
}

impl ShamirDb {
    pub(super) async fn authorize_function_replacement(
        &self,
        actor: &Actor,
        name: &str,
        existing: &QueryValue,
        new_security: Security,
    ) -> DbResult<()> {
        let path = ResourcePath::function(name);
        self.authorize_access(actor, &path, Action::Write)
            .await
            .map_err(|error| DbError::Function(error.to_string()))?;

        let owner = ResourceMeta::owner_field(existing);
        if owner
            .as_ref()
            .is_some_and(|owner| actor.to_owner_id() == owner.to_owner_id())
            || self
                .authorize_access(actor, &ResourcePath::Root, Action::Manage)
                .await
                .is_ok()
        {
            return Ok(());
        }
        if owner.is_none() {
            return Err(DbError::Function(
                crate::access::AccessError {
                    actor: actor.clone(),
                    path: path.to_string(),
                    action: Action::Write,
                }
                .to_string(),
            ));
        }

        let current = FunctionMeta::try_from_record(existing)
            .map_err(|error| DbError::Function(error.to_string()))?;
        let existing_meta = ResourceMeta::from_record(existing);
        if current.security == Security::Invoker
            && !Mode::is_setuid(existing_meta.mode)
            && new_security == Security::Invoker
        {
            return Ok(());
        }
        Err(DbError::Function(
            crate::access::AccessError {
                actor: actor.clone(),
                path: path.to_string(),
                action: Action::Write,
            }
            .to_string(),
        ))
    }
}

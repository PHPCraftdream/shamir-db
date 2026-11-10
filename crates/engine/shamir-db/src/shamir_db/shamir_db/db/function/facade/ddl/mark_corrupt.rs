use std::sync::Arc;

#[cfg(test)]
use crate::shamir_db::shamir_db::core::FunctionDdlHookPoint;
use crate::shamir_db::shamir_db::ShamirDb;
use crate::{DbError, DbResult};

use super::super::super::artifact::artifact_id_from_record;
use super::super::super::boot::{decode_stored_bytes, verify_stored_hash, HashVerdict};
use super::super::super::types::{FunctionState, UnavailableFunction};

impl ShamirDb {
    /// Serialize corruption marking, then re-check the current catalogue row
    /// before replacing the matching registry artifact.
    pub(in crate::shamir_db::shamir_db::db::function::facade) async fn mark_function_corrupt(
        &self,
        name: &str,
        expected_artifact_id: Option<&str>,
    ) -> DbResult<DbError> {
        #[cfg(test)]
        self.function_ddl_test_hook(FunctionDdlHookPoint::MarkCorruptBeforeLock)
            .await;
        let _function_guard = self.function_ddl_locks.lock(name).await;
        self.mark_function_corrupt_locked(name, expected_artifact_id)
            .await
    }

    /// Mark corruption while the caller holds this function's DDL lock.
    pub(in crate::shamir_db::shamir_db::db::function::facade) async fn mark_function_corrupt_locked(
        &self,
        name: &str,
        expected_artifact_id: Option<&str>,
    ) -> DbResult<DbError> {
        let changed = || DbError::Function("function changed during operation".to_string());
        let Some(record) = self.system_store.load_function(name).await? else {
            return Ok(changed());
        };
        let artifact_id = artifact_id_from_record(&record)
            .map_err(|error| DbError::Function(format!("invalid function artifact id: {error}")))?;
        if artifact_id.as_deref() != expected_artifact_id {
            return Ok(changed());
        }
        #[cfg(test)]
        self.function_ddl_test_hook(
            FunctionDdlHookPoint::MarkCorruptAfterRowReadBeforeRegistryWrite,
        )
        .await;
        let bytes = match decode_stored_bytes(&record) {
            Some(bytes) => bytes.map_err(DbError::Function)?,
            None => return Ok(changed()),
        };
        let (expected, actual) = match verify_stored_hash(&record, &bytes) {
            HashVerdict::Mismatch { expected, actual } => (expected, actual),
            HashVerdict::Match(_) | HashVerdict::Unknown => return Ok(changed()),
        };
        let state = FunctionState::Corrupt { expected, actual };
        let msg = state
            .unavailable_message(name)
            .unwrap_or_else(|| format!("function '{name}' is unavailable"));
        if !self.functions.replace_if_artifact_id(
            name,
            Arc::new(UnavailableFunction::new(name, state.clone())),
            expected_artifact_id,
        ) {
            return Ok(changed());
        }
        self.function_state.insert(name.to_string(), state);
        Ok(DbError::Function(msg))
    }
}

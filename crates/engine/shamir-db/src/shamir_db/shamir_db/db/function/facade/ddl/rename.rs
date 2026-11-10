use crate::access::{Action, Actor, ResourceMeta};
#[cfg(test)]
use crate::shamir_db::shamir_db::core::FunctionDdlHookPoint;
use crate::shamir_db::shamir_db::ShamirDb;
use crate::{DbError, DbResult};
use shamir_types::types::value::QueryValue;

impl ShamirDb {
    pub(in crate::shamir_db::shamir_db::db::function) async fn rename_function_catalogue_first(
        &self,
        from: &str,
        to: &str,
        actor: &Actor,
    ) -> DbResult<()> {
        if self.system_store.load_function(to).await?.is_some() {
            return Err(DbError::Function(format!(
                "function '{}' already exists",
                to
            )));
        }
        let old_record = self.system_store.load_function(from).await?;
        self.authorize_function_row_action(actor, from, old_record.as_ref(), Action::Write)
            .await
            .map_err(|error| DbError::Function(error.to_string()))?;
        #[cfg(test)]
        self.function_ddl_test_hook(FunctionDdlHookPoint::RenameAfterRowReadBeforeWrite)
            .await;

        // Match FunctionRegistry::rename's pre-write errors so no catalogue
        // change occurs for a missing source or occupied registry target.
        if self.functions.capture(from).is_none() {
            return Err(DbError::Function(format!("function not found: {from}")));
        }
        if self.functions.contains(to) {
            return Err(DbError::Function(format!(
                "function already registered: {to}"
            )));
        }

        if let Some(original_record) = old_record.as_ref() {
            let existing_meta = ResourceMeta::from_record(original_record);
            let mut renamed_record = original_record.clone();
            if let QueryValue::Map(ref mut map) = renamed_record {
                map.insert("name".to_string(), QueryValue::Str(to.to_string()));
            }
            self.system_store
                .save_function(to, &renamed_record, &existing_meta)
                .await?;
            if let Err(original_error) = self.system_store.remove_function(from).await {
                if let Err(compensation_error) = self.system_store.remove_function(to).await {
                    log::warn!(
                        "function rename {from} -> {to} source removal failed ({original_error}); destination compensation also failed ({compensation_error})"
                    );
                }
                return Err(original_error);
            }
        }

        #[cfg(test)]
        self.function_ddl_test_hook(
            FunctionDdlHookPoint::RenameAfterCatalogueWritesBeforeRegistryRename,
        )
        .await;
        if let Err(registry_error) = self.functions.rename(from, to) {
            let original_error = DbError::Function(registry_error.to_string());
            if let Some(original_record) = old_record.as_ref() {
                let original_meta = ResourceMeta::from_record(original_record);
                match self
                    .system_store
                    .save_function(from, original_record, &original_meta)
                    .await
                {
                    Ok(()) => {
                        if let Err(compensation_error) = self.system_store.remove_function(to).await
                        {
                            log::warn!(
                                "function rename {from} -> {to} registry rename failed ({original_error}); restored source row but destination cleanup failed ({compensation_error})"
                            );
                        }
                    }
                    Err(restore_error) => match self.system_store.load_function(from).await {
                        Ok(Some(restored_record)) if &restored_record == original_record => {
                            match self.system_store.remove_function(to).await {
                                Ok(()) => log::error!(
                                    "function rename {from} -> {to} registry rename failed ({original_error}); restoring the source row reported failure ({restore_error}), but the original source row is present and destination cleanup succeeded"
                                ),
                                Err(cleanup_error) => log::error!(
                                    "function rename {from} -> {to} registry rename failed ({original_error}); restoring the source row reported failure ({restore_error}) and destination cleanup failed ({cleanup_error})"
                                ),
                            }
                        }
                        Ok(Some(_)) => log::error!(
                            "function rename {from} -> {to} registry rename failed ({original_error}); restoring the source row failed ({restore_error}) and the source row does not match the original; destination row retained"
                        ),
                        Ok(None) => log::error!(
                            "function rename {from} -> {to} registry rename failed ({original_error}); restoring the source row failed ({restore_error}); destination row retained as the only durable copy"
                        ),
                        Err(check_error) => log::error!(
                            "function rename {from} -> {to} registry rename failed ({original_error}); restoring the source row failed ({restore_error}) and source verification failed ({check_error}); destination row retained"
                        ),
                    },
                }
            }
            return Err(original_error);
        }

        // Move in-memory metadata only after both catalogue writes and the
        // registry rename succeed; preserve any prior destination metadata
        // on the pre-write error paths above.
        let source_meta = self.function_meta(from);
        if let Some(meta) = source_meta {
            self.function_meta.insert(to.to_string(), meta);
        }
        let _ = self.function_meta.remove(from);
        if let Some((_, state)) = self.function_state.remove(from) {
            self.function_state.insert(to.to_string(), state);
        }
        Ok(())
    }
}

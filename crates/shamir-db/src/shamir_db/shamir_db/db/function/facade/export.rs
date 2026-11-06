use crate::access::{Action, Actor, ResourcePath};
use crate::shamir_db::shamir_db::{ArtifactKind, ShamirDb};
use crate::{DbError, DbResult};
use shamir_engine::function::ModuleHash;

use super::super::boot::{decode_stored_bytes, verify_stored_hash, HashVerdict};
use super::super::types::FunctionExport;

impl ShamirDb {
    /// Export a function's stored artifact as the system actor.
    pub async fn export_function(&self, name: &str) -> DbResult<FunctionExport> {
        self.export_function_as(name, Actor::System).await
    }

    /// Return the stored module bytes, source and hash after verifying them.
    /// A hash mismatch marks the function corrupt and fails closed.
    pub async fn export_function_as(&self, name: &str, actor: Actor) -> DbResult<FunctionExport> {
        self.authorize_access(
            &actor,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Write,
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;
        let row = self
            .system_store
            .load_function(name)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("function '{name}' not found")))?;
        let no_module = || DbError::Function(format!("function '{name}' has no exportable module"));
        if ArtifactKind::from_record(&row) == ArtifactKind::Native {
            return Err(no_module());
        }
        let bytes = decode_stored_bytes(&row)
            .ok_or_else(no_module)?
            .map_err(DbError::Function)?;
        let hash = match verify_stored_hash(&row, &bytes) {
            HashVerdict::Mismatch { expected, actual } => {
                return Err(self.mark_function_corrupt(name, expected, actual));
            }
            HashVerdict::Match(h) => h.to_hex(),
            HashVerdict::Unknown => ModuleHash::of(&bytes).to_hex(),
        };
        Ok(FunctionExport {
            bytes,
            source: row["source"].as_str().map(str::to_string),
            hash,
        })
    }
}

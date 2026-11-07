use crate::access::{Action, Actor, ResourcePath};
use crate::shamir_db::shamir_db::{ArtifactKind, ShamirDb};
use crate::{DbError, DbResult};
use shamir_engine::function::TierMode;

use super::super::boot::{decode_stored_bytes, stored_optimize, verify_stored_hash, HashVerdict};
use super::super::types::{FunctionDescription, FunctionState};

impl ShamirDb {
    /// Describe a function as the system actor.
    pub async fn describe_function(&self, name: &str) -> DbResult<FunctionDescription> {
        self.describe_function_as(name, Actor::System).await
    }

    /// Introspect a function. Authorization precedes any existence probe.
    pub async fn describe_function_as(
        &self,
        name: &str,
        actor: Actor,
    ) -> DbResult<FunctionDescription> {
        self.authorize_access(
            &actor,
            &ResourcePath::Function {
                name: name.to_string(),
            },
            Action::Read,
        )
        .await
        .map_err(|e| DbError::Function(e.to_string()))?;
        let function = self
            .functions
            .get(name)
            .ok_or_else(|| DbError::NotFound(format!("function '{name}' not found")))?;
        let row = self.system_store.load_function(name).await?;
        let state = self
            .function_state
            .get(name)
            .map(|s| s.value().clone())
            .unwrap_or(FunctionState::Loaded);

        let Some(row) = row.filter(|r| ArtifactKind::from_record(r) != ArtifactKind::Native) else {
            return Ok(FunctionDescription {
                name: name.to_string(),
                lang: "native".to_string(),
                origin_claim: None,
                abi_version: None,
                hash: None,
                byte_size: None,
                optimize: TierMode::Auto,
                tier_report: function.tier_report(),
                state,
            });
        };

        let bytes = decode_stored_bytes(&row).and_then(Result::ok);
        let hash = match row["module_hash"].as_str() {
            Some(h) => Some(h.to_string()),
            None => bytes
                .as_deref()
                .and_then(|b| match verify_stored_hash(&row, b) {
                    HashVerdict::Match(h) => Some(h.to_hex()),
                    _ => None,
                }),
        };
        Ok(FunctionDescription {
            name: name.to_string(),
            lang: row["lang"].as_str().unwrap_or("wasm").to_string(),
            origin_claim: row["origin"].as_str().map(str::to_string),
            abi_version: row["abi"].as_i64().and_then(|a| u32::try_from(a).ok()),
            hash,
            byte_size: bytes.as_ref().map(Vec::len),
            optimize: stored_optimize(&row),
            tier_report: function.tier_report(),
            state,
        })
    }
}

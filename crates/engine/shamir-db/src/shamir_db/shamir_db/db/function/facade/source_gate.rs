use crate::access::{Action, Actor, ResourceMeta, ResourcePath};
use crate::shamir_db::shamir_db::access::AclMemo;
use crate::shamir_db::shamir_db::{ArtifactKind, FunctionSource, ShamirDb};
use crate::{DbError, DbResult};
use shamir_engine::function::{
    compile_rust_source_blocking, create_shamir_source, CompileOptions, FunctionError, ModuleHash,
    ModuleReport, ShamirStage, TierMode, WasmFunction, WasmLimits,
};
use shamir_types::types::value::QueryValue;
use std::collections::BTreeMap;

use super::super::invocation::strict_nested_meta;

fn callable_row_metadata_is_valid(record: &QueryValue) -> bool {
    strict_nested_meta(record).is_ok() && ResourceMeta::owner_field(record).is_some()
}

impl ShamirDb {
    /// Shared authorization for `shamir` language sources. Shamir compilation
    /// is in-process under the engine's compile gate (no external toolchain),
    /// but the same WasmCompiler Execute gate and source-size cap as the rust
    /// path are applied so both languages cost the same permissions.
    pub(in crate::shamir_db::shamir_db) async fn authorize_shamir_source(
        &self,
        src: &str,
        actor: &Actor,
    ) -> DbResult<()> {
        self.authorize_access(actor, &ResourcePath::WasmCompiler, Action::Execute)
            .await
            .map_err(|e| DbError::Function(e.to_string()))?;
        let limit = self.wasm_engine.admission_limits().max_source_bytes;
        if src.len() > limit {
            return Err(DbError::Function(format!(
                "source is {} bytes, limit is {}",
                src.len(),
                limit
            )));
        }
        Ok(())
    }

    /// Snapshot persisted signatures from hash-identified `shamir` WASM rows.
    /// Runtime resolution remains responsible for artifact coherence.
    pub(super) async fn callable_catalog_snapshot(
        &self,
        actor: &Actor,
        source: &str,
    ) -> DbResult<BTreeMap<String, Vec<u8>>> {
        if !source.contains("call") {
            return Ok(BTreeMap::new());
        }
        let records = self.system_store.load_functions().await?;
        let settings = if matches!(actor, Actor::System | Actor::Admin(_)) {
            None
        } else {
            Some(self.system_store.load_invocation_settings().await)
        };
        let memo = AclMemo::default();
        let mut catalog = BTreeMap::new();
        for record in &records {
            let Some(name) = record.get("name").and_then(QueryValue::as_str) else {
                continue;
            };
            if record.get("lang").and_then(QueryValue::as_str) != Some("shamir")
                || ArtifactKind::from_record(record) != ArtifactKind::Wasm
                || !callable_row_metadata_is_valid(record)
            {
                continue;
            }
            if self
                .authorize_invocation_with_settings(
                    name,
                    actor,
                    Some(record),
                    settings.as_ref().map(Result::as_ref),
                    &memo,
                )
                .await
                .is_err()
            {
                continue;
            }
            let Some(QueryValue::Bin(signature)) = record.get("call_signature") else {
                continue;
            };
            let Some(module_hash) = record.get("module_hash").and_then(QueryValue::as_str) else {
                continue;
            };
            if ModuleHash::from_hex(module_hash).is_err() || signature.is_empty() {
                continue;
            }
            catalog.insert(name.to_string(), signature.clone());
        }
        Ok(catalog)
    }

    /// Turn a [`FunctionSource`] into `(wasm bytes, lang tag, source text,
    /// ready-built function, callable signature)`.
    ///
    /// For `Source`, authorizes the compiler and caps the source size before
    /// the compiler process is spawned.
    ///
    /// For `Shamir`, the host source service compiles AND gate-admits in one
    /// step, so the fourth element carries the ready-built
    /// `(WasmFunction, ModuleReport)` — callers skip
    /// `from_binary_admitted_blocking` to avoid compiling twice under two
    /// separate gate acquisitions. The `Wasm`/`Source` arms return `None`.
    pub(in crate::shamir_db::shamir_db) async fn materialize_function_source(
        &self,
        source: FunctionSource<'_>,
        actor: &Actor,
        optimize: TierMode,
    ) -> DbResult<(
        Vec<u8>,
        &'static str,
        Option<String>,
        Option<(WasmFunction, ModuleReport)>,
        Option<Vec<u8>>,
    )> {
        match source {
            FunctionSource::Wasm(bytes) => Ok((bytes.to_vec(), "wasm", None, None, None)),
            FunctionSource::Source(src) => {
                // Task #607: compiling Rust source into WASM runs a host
                // compiler process — gate it as a separate POSIX-style
                // permission (Execute bit on the WasmCompiler singleton),
                // not folded into bare FunctionNamespace Create. Mirrors the
                // secret_grants escalation pattern in create.
                self.authorize_access(actor, &ResourcePath::WasmCompiler, Action::Execute)
                    .await
                    .map_err(|e| DbError::Function(e.to_string()))?;
                let limit = self.wasm_engine.admission_limits().max_source_bytes;
                if src.len() > limit {
                    return Err(DbError::Function(format!(
                        "source is {} bytes, limit is {}",
                        src.len(),
                        limit
                    )));
                }
                let compiled = compile_rust_source_blocking(src)
                    .await
                    .map_err(|e| match e {
                        FunctionError::ToolchainUnavailable(msg) => {
                            DbError::Function(format!("toolchain unavailable: {}", msg))
                        }
                        other => DbError::Function(other.to_string()),
                    })?;
                Ok((compiled, "rust", Some(src.to_string()), None, None))
            }
            FunctionSource::Shamir(src) => {
                self.authorize_shamir_source(src, actor).await?;
                let mut options = CompileOptions {
                    stage: ShamirStage::Procedures,
                    ..CompileOptions::default()
                };
                options.callable_catalog = self.callable_catalog_snapshot(actor, src).await?;
                let (wf, wasm, report, signature) = create_shamir_source(
                    self.wasm_engine.clone(),
                    src.to_string(),
                    options,
                    WasmLimits::default(),
                    optimize,
                )
                .await
                .map_err(|e| DbError::Function(e.to_string()))?;
                Ok((
                    wasm,
                    "shamir",
                    Some(src.to_string()),
                    Some((wf, report)),
                    signature,
                ))
            }
        }
    }
}

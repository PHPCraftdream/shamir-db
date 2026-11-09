use crate::access::{Action, Actor, ResourcePath};
use crate::shamir_db::shamir_db::{FunctionSource, ShamirDb};
use crate::{DbError, DbResult};
use shamir_engine::function::{
    compile_rust_source_blocking, create_shamir_source, CompileOptions, FunctionError,
    ModuleReport, TierMode, WasmFunction, WasmLimits,
};

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

    /// Turn a [`FunctionSource`] into `(wasm bytes, lang tag, source text,
    /// ready-built function)`.
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
    )> {
        match source {
            FunctionSource::Wasm(bytes) => Ok((bytes.to_vec(), "wasm", None, None)),
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
                Ok((compiled, "rust", Some(src.to_string()), None))
            }
            FunctionSource::Shamir(src) => {
                self.authorize_shamir_source(src, actor).await?;
                let (wf, wasm, report) = create_shamir_source(
                    self.wasm_engine.clone(),
                    src.to_string(),
                    CompileOptions::default(),
                    WasmLimits::default(),
                    optimize,
                )
                .await
                .map_err(|e| DbError::Function(e.to_string()))?;
                Ok((wasm, "shamir", Some(src.to_string()), Some((wf, report))))
            }
        }
    }
}

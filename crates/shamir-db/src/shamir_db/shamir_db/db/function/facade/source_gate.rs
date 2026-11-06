use crate::access::{Action, Actor, ResourcePath};
use crate::shamir_db::shamir_db::{FunctionSource, ShamirDb};
use crate::{DbError, DbResult};
use shamir_engine::function::{compile_rust_source_blocking, FunctionError};

impl ShamirDb {
    /// Turn a [`FunctionSource`] into `(wasm bytes, lang tag, source text)`.
    ///
    /// For `Source`, authorizes the compiler and caps the source size before
    /// the compiler process is spawned.
    pub(in crate::shamir_db::shamir_db) async fn materialize_function_source(
        &self,
        source: FunctionSource<'_>,
        actor: &Actor,
    ) -> DbResult<(Vec<u8>, &'static str, Option<String>)> {
        match source {
            FunctionSource::Wasm(bytes) => Ok((bytes.to_vec(), "wasm", None)),
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
                Ok((compiled, "rust", Some(src.to_string())))
            }
        }
    }
}

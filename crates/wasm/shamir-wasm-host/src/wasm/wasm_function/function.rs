use crate::runtime::error::{FnResult, FunctionError};
use crate::wasm::admission::{admit_module, run_gated_compile, AdmissionPolicy, ModuleReport};
use crate::wasm::engine::tier_upgrade::{FunctionShared, FunctionSlot};
use crate::wasm::engine::{Tier, TierMode, TierReport, WasmEngine, WasmLimits};
use std::sync::Arc;
use std::time::Instant;

use super::linker::build_instance_pre;

// ── WasmFunction ─────────────────────────────────────────────────────

/// A [`ShamirFunction`] backed by a compiled WebAssembly module.
///
/// Created from either WAT text ([`WasmFunction::from_wat`]) or binary
/// `.wasm` bytes ([`WasmFunction::from_binary`]).
///
/// The compiled form lives in an RCU slot (`FunctionSlot`) pairing the
/// engine the module was compiled for with its `InstancePre` and the tier
/// it resides on. Creation compiles the fast tier; once the call counter
/// crosses `TierOptions::hot_calls`, one bounded background job swaps in an
/// optimized-tier slot.
pub struct WasmFunction {
    pub(super) shared: Arc<FunctionShared>,
}

impl WasmFunction {
    /// Compile a WAT text module into a `WasmFunction`.
    ///
    /// Runs the pre-instantiation admission gate
    /// ([`admit_module`] with [`AdmissionPolicy::Load`]) on the WAT's binary encoding before
    /// `wasmtime::Module::new`/`instantiate_pre` ever see it — see
    /// `admission::sanitizer` module docs for why this is a real, if modest,
    /// defense-in-depth control (fail-fast + an explicit auditable
    /// allowlist) rather than a redundant re-check of what the linker
    /// already rejects for free.
    /// Behaves as [`TierMode::Auto`].
    pub fn from_wat(engine: Arc<WasmEngine>, wat: &str, limits: WasmLimits) -> FnResult<Self> {
        Self::from_wat_with_mode(engine, wat, limits, TierMode::Auto)
    }

    /// [`WasmFunction::from_wat`] with an explicit tier mode.
    pub fn from_wat_with_mode(
        engine: Arc<WasmEngine>,
        wat: &str,
        limits: WasmLimits,
        mode: TierMode,
    ) -> FnResult<Self> {
        engine.validate_limits(&limits)?;
        let wasm_bytes = wat::parse_str(wat)
            .map_err(|e| FunctionError::Compute(format!("WAT parse error: {e}")))?;
        admit_module(&wasm_bytes, &AdmissionPolicy::Load)?;
        Self::from_sanitized(engine, wasm_bytes.into(), limits, mode, "WAT compile error")
    }

    /// Compile a `.wasm` binary into a `WasmFunction`.
    ///
    /// Runs the pre-instantiation admission gate
    /// ([`admit_module`] with [`AdmissionPolicy::Load`]) on the raw bytes before
    /// `wasmtime::Module::from_binary`/`instantiate_pre` ever see them —
    /// this is the path both the compile-on-DDL pipeline
    /// (`compile_rust_source`'s output) and durable-catalogue recovery
    /// (loading previously-stored `.wasm` bytes back at startup) both go
    /// through, so every production call site is covered by this single
    /// choke point.
    ///
    /// NOTE: despite the `wasm_or_cwasm_bytes` name (inherited from
    /// `wasmtime::Module::from_binary`, which accepts either raw `.wasm`
    /// or a pre-serialized `.cwasm` artifact), no call site in this
    /// codebase produces or loads `.cwasm` bytes today — `.cwasm` AOT
    /// caching is aspirational (see the crate's module docs). The
    /// sanitizer parses raw wasm section bytes via `wasmparser` and would
    /// fail to parse a genuine `.cwasm` blob; if `.cwasm` loading is ever
    /// added, this call site will need to special-case it (e.g. skip the
    /// sanitizer for a detected `.cwasm` header, relying on the fact that
    /// `.cwasm` can only be produced by our own prior `Module::serialize`
    /// call over an already-sanitized module).
    /// Behaves as [`TierMode::Auto`].
    pub fn from_binary(
        engine: Arc<WasmEngine>,
        wasm_or_cwasm_bytes: &[u8],
        limits: WasmLimits,
    ) -> FnResult<Self> {
        Self::from_binary_with_mode(engine, wasm_or_cwasm_bytes, limits, TierMode::Auto)
    }

    /// [`WasmFunction::from_binary`] with an explicit tier mode.
    pub fn from_binary_with_mode(
        engine: Arc<WasmEngine>,
        wasm_or_cwasm_bytes: &[u8],
        limits: WasmLimits,
        mode: TierMode,
    ) -> FnResult<Self> {
        engine.validate_limits(&limits)?;
        admit_module(wasm_or_cwasm_bytes, &AdmissionPolicy::Load)?;
        Self::from_sanitized(
            engine,
            wasm_or_cwasm_bytes.into(),
            limits,
            mode,
            "WASM compile error",
        )
    }
    /// Async counterpart of [`WasmFunction::from_binary`].
    ///
    /// Cranelift compilation takes tens to hundreds of milliseconds of pure
    /// CPU; async callers must not run it inline on a worker thread. This
    /// moves the synchronous [`WasmFunction::from_binary`] pipeline —
    /// sanitizer, compile, instantiation — onto `tokio::task::spawn_blocking`.
    /// Behaviour and error values are identical to `from_binary`; behaves as
    /// [`TierMode::Auto`].
    pub async fn from_binary_blocking(
        engine: Arc<WasmEngine>,
        wasm_or_cwasm_bytes: &[u8],
        limits: WasmLimits,
    ) -> FnResult<Self> {
        Self::from_binary_blocking_with_mode(engine, wasm_or_cwasm_bytes, limits, TierMode::Auto)
            .await
    }

    /// [`WasmFunction::from_binary_blocking`] with an explicit tier mode.
    pub async fn from_binary_blocking_with_mode(
        engine: Arc<WasmEngine>,
        wasm_or_cwasm_bytes: &[u8],
        limits: WasmLimits,
        mode: TierMode,
    ) -> FnResult<Self> {
        let wasm = wasm_or_cwasm_bytes.to_vec();
        run_compile_blocking(move || Self::from_binary_with_mode(engine, &wasm, limits, mode)).await
    }

    /// Admits the bytes under `policy` (returning the [`ModuleReport`]) and
    /// compiles on the blocking pool under the engine's compile gate
    /// (concurrency bound, plus the compile time cap under
    /// [`AdmissionPolicy::Admission`]), so neither the scan nor the compile
    /// runs on an async worker thread.
    pub async fn from_binary_admitted_blocking(
        engine: Arc<WasmEngine>,
        wasm_or_cwasm_bytes: &[u8],
        limits: WasmLimits,
        mode: TierMode,
        policy: AdmissionPolicy,
    ) -> FnResult<(Self, ModuleReport)> {
        engine.validate_limits(&limits)?;
        let wasm = wasm_or_cwasm_bytes.to_vec();
        let gate = Arc::clone(&engine);
        run_gated_compile(&gate, &policy, move |_permit| {
            let report = admit_module(&wasm, &policy)?;
            let function =
                Self::from_sanitized(engine, wasm.into(), limits, mode, "WASM compile error")?;
            Ok((function, report))
        })
        .await
    }

    /// Assembles a function from already-admitted bytes WITHOUT acquiring a
    /// compile-gate permit: the shamir source service owns the single permit
    /// and calls this inside its gated closure. Same pipeline as the inner
    /// half of [`Self::from_binary_admitted_blocking`].
    pub(in crate::wasm) fn from_sanitized_ungated(
        engine: Arc<WasmEngine>,
        wasm: Arc<[u8]>,
        limits: WasmLimits,
        mode: TierMode,
    ) -> FnResult<Self> {
        Self::from_sanitized(engine, wasm, limits, mode, "shamir compile error")
    }

    /// The compilation tier the function's live slot is resident on.
    pub fn tier(&self) -> Tier {
        self.shared.tier()
    }

    /// The tier mode fixed at construction.
    pub fn mode(&self) -> TierMode {
        self.shared.mode()
    }

    /// Snapshot of the live tier, the mode, the upgrade call counter and the upgrade state.
    pub fn tier_report(&self) -> TierReport {
        self.shared.report()
    }

    /// Assemble the function from already-validated bytes (sanitizer and
    /// limit checks ran once, before any compile).
    fn from_sanitized(
        engine: Arc<WasmEngine>,
        wasm: Arc<[u8]>,
        limits: WasmLimits,
        mode: TierMode,
        compile_context: &'static str,
    ) -> FnResult<Self> {
        let slot = Self::compile_initial_slot(&engine, &wasm, mode, compile_context)?;
        Ok(Self {
            shared: Arc::new(FunctionShared::new(
                engine,
                wasm,
                limits,
                mode,
                slot,
                build_instance_pre,
            )),
        })
    }

    /// Build the creation-time slot for `mode`, logging the tier decision
    /// (tier, compile time, module size) at `info`.
    ///
    /// `Auto`: the fast tier synchronously, falling back to the optimized
    /// tier (logged at `info`, not an error) when the fast compile fails or
    /// the fast engine is absent. `Now`: the optimized tier, no upgrade.
    /// `Never`: the fast tier only (the optimized tier when this engine has no fast tier).
    fn compile_initial_slot(
        engine: &WasmEngine,
        wasm: &[u8],
        mode: TierMode,
        compile_context: &'static str,
    ) -> FnResult<FunctionSlot> {
        let started = Instant::now();
        let (engine_handle, module, tier, decision) = match mode {
            TierMode::Now => {
                let module = engine
                    .compile_optimized(wasm)
                    .map_err(|e| FunctionError::Compute(format!("{compile_context}: {e}")))?;
                (
                    engine.optimized_engine().clone(),
                    module,
                    Tier::Optimized,
                    "optimized (TierMode::Now)",
                )
            }
            TierMode::Never => match engine.fast_engine().cloned() {
                Some(fast_engine) => {
                    let module = engine
                        .compile_fast(wasm)
                        .map_err(|e| FunctionError::Compute(format!("{compile_context}: {e}")))?;
                    (fast_engine, module, Tier::Fast, "fast (TierMode::Never)")
                }
                None => {
                    // No fast tier on this engine (tiering disabled or the fast engine could
                    // not be built): there is nothing to stay on, the optimized tier hosts it.
                    let module = engine
                        .compile_optimized(wasm)
                        .map_err(|e| FunctionError::Compute(format!("{compile_context}: {e}")))?;
                    (
                        engine.optimized_engine().clone(),
                        module,
                        Tier::Optimized,
                        "optimized (Never, fast tier unavailable)",
                    )
                }
            },
            TierMode::Auto => match engine.fast_engine().cloned() {
                Some(fast_engine) => match engine.compile_fast(wasm) {
                    Ok(module) => (fast_engine, module, Tier::Fast, "fast (Auto)"),
                    Err(fast_error) => {
                        // Not an error for the caller: the optimized tier
                        // hosts the slot from the first call on.
                        log::info!(
                            "wasm fast tier compile failed ({fast_error}); \
                             the optimized tier hosts the slot"
                        );
                        let module = engine.compile_optimized(wasm).map_err(|e| {
                            FunctionError::Compute(format!("{compile_context}: {e}"))
                        })?;
                        (
                            engine.optimized_engine().clone(),
                            module,
                            Tier::Optimized,
                            "optimized (Auto fast-compile fallback)",
                        )
                    }
                },
                None => {
                    let module = engine
                        .compile_optimized(wasm)
                        .map_err(|e| FunctionError::Compute(format!("{compile_context}: {e}")))?;
                    (
                        engine.optimized_engine().clone(),
                        module,
                        Tier::Optimized,
                        "optimized (Auto, fast engine unavailable)",
                    )
                }
            },
        };
        let instance_pre = build_instance_pre(&engine_handle, &module)?;
        log::info!(
            "wasm function slot on the {decision} tier compiled in {:?}, {} module bytes",
            started.elapsed(),
            wasm.len()
        );
        Ok(FunctionSlot {
            engine: engine_handle,
            instance_pre,
            tier,
        })
    }
}

/// Run a CPU-bound compile step on tokio's blocking thread pool.
///
/// Shared by [`WasmFunction::from_binary_blocking`] (and unit tests):
/// moves the closure onto `tokio::task::spawn_blocking` and maps a
/// `JoinError` (panic or cancellation of the blocking task) to
/// [`FunctionError::Compute`], the same variant a failed compile produces,
/// so callers keep a single error type.
pub(crate) async fn run_compile_blocking<F, T>(f: F) -> FnResult<T>
where
    F: FnOnce() -> FnResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| FunctionError::Compute(format!("blocking compile join failure: {e}")))?
}

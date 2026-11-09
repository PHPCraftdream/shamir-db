//! The shamir source service: compile `shamir-lang` source into an admitted,
//! hosted [`WasmFunction`], or dry-run it without persisting anything.
//!
//! # PATH DEVIATION
//!
//! The design (section 6) mandates `src/wasm/lang_compile.rs`, but `wasm/`
//! is already at its 7-direct-entries layout cap; the module therefore lives
//! inside the compile-gate folder (`gate/` has room, going 3→4). The
//! re-export surface through `gate/mod.rs`, `admission/mod.rs`, `wasm/mod.rs`
//! and `lib.rs` is exactly as designed.
//!
//! # Lifecycle: exactly ONE compile-gate permit
//!
//! Both factories derive the admission policy from the engine (never the
//! caller's `options.admission`), run `WasmEngine::validate_limits` first
//! (mirroring `from_binary_admitted_blocking`), and wrap the entire
//! compile+admit+build body in `run_gated_compile` — the same single-permit
//! discipline as `check_module` and `WasmFunction::from_binary_admitted_blocking`,
//! which are NEVER nested here (each owns its own gate). The gate's 10-second
//! async timeout starts AFTER the permit is granted; it cannot cancel the
//! worker and no output is persisted on expiry.
//!
//! # Permit acquire/hold/drop points
//!
//! * ACQUIRE: inside `run_gated_compile`, before the blocking job starts.
//! * HOLD: the gate passes the owned permit into the job closure, which
//!   moves it onward into the fixed-stack worker thread's closure.
//! * DROP: the END OF THE WORKER THREAD's closure — the design rule
//!   "retain the permit until the real worker exits, not merely until its
//!   join handle is dropped". A timeout rejects the caller but never
//!   cancels the running worker (which still holds the permit) and never
//!   persists output.
//! * SPAWN FAILURE: the worker never started, so the permit drops with the
//!   blocking closure — the retention rule is about a RUNNING worker.

use crate::runtime::error::{FnResult, FunctionError};
use crate::wasm::admission::gate::check::check_module_ungated;
use crate::wasm::admission::gate::compile_gate::run_gated_compile;
use crate::wasm::admission::{admit_module, AdmissionPolicy, ModuleReport};
use crate::wasm::engine::{TierMode, WasmEngine, WasmLimits};
use crate::wasm::wasm_function::WasmFunction;
use shamir_lang::{CompileError, CompileOptions};
use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OwnedSemaphorePermit;

/// Fixed-stack worker size for the compiler (design sections 5/6). Stack
/// size is not isolation: `catch_unwind` cannot protect against stack
/// overflow, so the real bound is the compiler's depth limits.
const WORKER_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Rendered-error bound: compiler diagnostics are individually capped at
/// 1024 bytes, so the render cap is eight full-size diagnostic lines;
/// truncation cuts at a UTF-8 char boundary at most 12 bytes below the cap
/// to leave room for the `" [truncated]"` marker.
const RENDERED_ERROR_MAX_BYTES: usize = 8 * 1024;

/// Compiles `source` and assembles a hosted [`WasmFunction`].
///
/// One gate permit covers compile + admission + build. The returned
/// [`ModuleReport`] is the host-side second scan; it equals the compiler's
/// self-admission report because the compile options' admission was derived
/// from the same engine limits. On success the caller owns both the
/// function and the emitted bytes.
pub async fn create_shamir_source(
    engine: Arc<WasmEngine>,
    source: String,
    options: CompileOptions,
    limits: WasmLimits,
    mode: TierMode,
) -> FnResult<(WasmFunction, Vec<u8>, ModuleReport)> {
    engine.validate_limits(&limits)?;
    // The caller's options.admission is never trusted: derive both the
    // compiler's self-admission limits and the gate policy from the engine.
    let mut options = options;
    options.admission = *engine.admission_limits();
    let policy = AdmissionPolicy::Admission(*engine.admission_limits());
    let gate = Arc::clone(&engine);
    run_gated_compile(&gate, &policy, move |permit| {
        let wasm = compile_in_worker(permit, &policy, source, options)?;
        // Host-side second admission (the compiler already self-admitted
        // under the same limits).
        let report = admit_module(&wasm, &policy)?;
        let function =
            WasmFunction::from_sanitized_ungated(engine, wasm.clone().into(), limits, mode)?;
        Ok((function, wasm, report))
    })
    .await
}

/// Compiles `source` and runs the full check pipeline (admission plus
/// uncached tier-specific compilation), persisting nothing.
pub async fn check_shamir_source(
    engine: Arc<WasmEngine>,
    source: String,
    options: CompileOptions,
    limits: WasmLimits,
    mode: TierMode,
) -> FnResult<ModuleReport> {
    engine.validate_limits(&limits)?;
    let mut options = options;
    options.admission = *engine.admission_limits();
    let policy = AdmissionPolicy::Admission(*engine.admission_limits());
    let gate = Arc::clone(&engine);
    run_gated_compile(&gate, &policy, move |permit| {
        let wasm = compile_in_worker(permit, &policy, source, options)?;
        check_module_ungated(&engine, &wasm, &policy, mode)
    })
    .await
}

/// Runs `shamir_lang::compile` on the fixed-stack worker while holding the
/// gate permit; returns the emitted bytes.
///
/// Permit lifecycle: the permit acquired by the enclosing
/// `run_gated_compile` is moved INTO the worker thread's closure here, so
/// its Drop point is the end of the worker thread's closure (the worker's
/// real exit), not this blocking closure's return — see the module docs.
/// The worker is never cancelled; on a time-cap expiry the caller is
/// rejected but the worker keeps running with the permit.
fn compile_in_worker(
    permit: OwnedSemaphorePermit,
    policy: &AdmissionPolicy,
    source: String,
    options: CompileOptions,
) -> FnResult<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<FnResult<Vec<u8>>>();
    let worker = std::thread::Builder::new()
        .name("shamir-lang-compile".to_string())
        .stack_size(WORKER_STACK_BYTES)
        .spawn(move || {
            // The gate permit's Drop point is the END of this closure — the
            // worker's real exit — not the blocking closure's return.
            let _permit = permit;
            let outcome =
                catch_unwind(AssertUnwindSafe(|| shamir_lang::compile(&source, &options)))
                    .map(|compiled| {
                        compiled
                            .map(|module| module.wasm)
                            .map_err(|error| render_compile_error(&error))
                    })
                    .unwrap_or_else(|payload| Err(render_panic(payload)));
            // If the receiver is gone (timeout expiry) the send fails and
            // is dropped: the worker still finishes and releases the permit.
            let _ = tx.send(outcome.map_err(FunctionError::Compute));
        })
        .map_err(|error| {
            // The worker never started, so the permit correctly drops with
            // this blocking closure (retention is about a RUNNING worker).
            FunctionError::Compute(format!(
                "failed to spawn the shamir compile worker: {error}"
            ))
        })?;
    // The receive cap comes from the policy, never from the caller:
    // `Load` does not inherit the Admission timeout (design section 6).
    let joined = match compile_time_cap(policy) {
        Some(cap) => match rx.recv_timeout(cap) {
            Ok(outcome) => outcome,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(FunctionError::Compute(format!(
                "shamir compile exceeded the compile time cap of {cap:?}; \
                 the module is rejected"
            ))),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(FunctionError::Compute(
                "I001: internal invariant: shamir compile worker closed its \
                 channel without a result"
                    .to_string(),
            )),
        },
        None => match rx.recv() {
            Ok(outcome) => outcome,
            Err(_) => Err(FunctionError::Compute(
                "I001: internal invariant: shamir compile worker closed its \
                 channel without a result"
                    .to_string(),
            )),
        },
    };
    // After a successful receive, rejoin the worker: catch_unwind already
    // converts compile panics, so a join failure here is defensive only.
    match worker.join() {
        Ok(()) => joined,
        Err(_) => Err(FunctionError::Compute(
            "I001: internal invariant: shamir compile worker join failed".to_string(),
        )),
    }
}

/// The blocking-side receive cap: [`AdmissionPolicy::Admission`] inherits
/// `AdmissionLimits::max_compile_time`; [`AdmissionPolicy::Load`] gets no
/// cap (design section 6).
fn compile_time_cap(policy: &AdmissionPolicy) -> Option<Duration> {
    match policy {
        AdmissionPolicy::Admission(limits) => Some(limits.max_compile_time),
        AdmissionPolicy::Load => None,
    }
}

/// Renders a [`CompileError`] deterministically: stable-code-prefixed,
/// bounded to [`RENDERED_ERROR_MAX_BYTES`], and independent of map order,
/// addresses, or platform paths.
fn render_compile_error(error: &CompileError) -> String {
    let rendered = match error {
        CompileError::Diagnostics(diagnostics) => diagnostics
            .iter()
            .map(|diagnostic| diagnostic.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        CompileError::Admission(admission_error) => {
            format!("B002: self-admission rejected the emitted module: {admission_error}")
        }
        CompileError::LimitExceeded {
            code,
            span,
            limit,
            observed,
        } => format!(
            "{code}:{}:{}: limit {limit} exceeded: {observed} observed",
            span.start.line, span.start.column
        ),
        CompileError::InternalInvariant(message) => {
            format!("I001: internal invariant: {message}")
        }
    };
    bounded(&rendered)
}

/// Cuts `text` to [`RENDERED_ERROR_MAX_BYTES`] at a UTF-8 char boundary at
/// most 12 bytes below the cap, appending `" [truncated]"` only when
/// truncation happened. The downward scan is manual: stable Rust has no
/// `floor_char_boundary`.
fn bounded(text: &str) -> String {
    if text.len() <= RENDERED_ERROR_MAX_BYTES {
        return text.to_string();
    }
    let mut end = RENDERED_ERROR_MAX_BYTES.saturating_sub(12);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} [truncated]", &text[..end])
}

/// Renders a worker panic payload deterministically; the panic is never
/// resumed — it becomes an `I001` internal-invariant error.
fn render_panic(payload: Box<dyn Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        format!("I001: internal invariant: shamir compile worker panicked: {text}")
    } else if let Some(text) = payload.downcast_ref::<String>() {
        format!("I001: internal invariant: shamir compile worker panicked: {text}")
    } else {
        "I001: internal invariant: shamir compile worker panicked".to_string()
    }
}

//! Pre-instantiation structural sanitizer for compiled `.wasm` artifacts
//! (task #545, CRITICAL residual).
//!
//! # Why this exists (investigation summary — see the task brief for the
//! full write-up)
//!
//! `wasmtime::Linker::instantiate_pre` (used by
//! (build_instance_pre in shamir-wasm-host's `wasm_function.rs`)) ALREADY rejects a module
//! that imports anything outside the linker's registered
//! `("shamir_host", <name>)` surface — confirmed empirically: a WAT module
//! importing `("evil", "syscall")` fails at `instantiate_pre` with
//! `unknown import: 'evil::syscall' has not been defined`, before any
//! wasm code ever runs. So this sanitizer does **not** close a hole the
//! linker leaves open. Its real value is:
//!
//! 1. **Fail-fast, cheaper than full compilation.** This module parses
//!    only the import section header via [`wasmparser`] — no Cranelift
//!    codegen, no full module validation. A malicious/malformed artifact
//!    with a smuggled import is rejected before `wasmtime::Module::new`/
//!    `from_binary` ever pays for compilation.
//! 2. **An explicit, auditable ABI allowlist independent of linker
//!    wiring.** [`SANCTIONED_HOST_IMPORTS`] is the single source of truth
//!    both this sanitizer AND `build_instance_pre`'s registrations are
//!    tested against (see the host crate's `src/tests/wasm/admission/sanitizer_tests.rs`), so the two
//!    surfaces cannot silently drift apart — a future change that
//!    broadens the linker's registered surface without a matching,
//!    deliberate update to this allowlist fails a dedicated sync test.
//!
//! Investigation also probed items the brief flagged as *possible*
//! additional structural gaps (module exports, memory minimum size,
//! `start` function abuse) and found none of them load-bearing:
//! - **Exports**: every entrypoint lookup in `wasm_function.rs` uses a
//!   fixed literal name (`"memory"`, `"shamir_alloc"`, `"shamir_call"`
//!   via `get_export`/`get_memory`/`get_typed_func`) — nothing iterates
//!   or dynamically dispatches on a module's export list, so an
//!   unexpected export cannot be "mistakenly invoked".
//! - **Memory minimum**: an oversized memory minimum (e.g. 65536 pages)
//!   already fails at `Module::new`/`instantiate_pre` under the pooling
//!   allocator ("module memory does not fit in pooling allocator
//!   requirements"), and — even with pooling disabled
//!   (`SHAMIR_WASM_NO_POOL=1`) — fails at `instantiate_async`
//!   ("memory minimum size ... exceeds memory limits") via the existing
//!   per-Store `ResourceLimiter`, strictly before any guest code runs.
//! - **`start` function abuse**: a `start` function runs inside
//!   `instantiate_async`, on the same `Store` whose fuel
//!   (`store.set_fuel`) and epoch deadline (`store.set_epoch_deadline`)
//!   are already configured before that call — so a busy-looping start
//!   function traps under the exact same budget as `shamir_call` would.
//!
//! None of these represent a gap the linker/Store leaves open, so this
//! sanitizer scopes to the one item with real, honest value: the import
//! allowlist.
//!
//! # Post-review hardening: component-encoded artifacts
//!
//! An adversarial review of the first implementation found that
//! `wasmparser`'s `component-model` feature (on by default, transitively
//! enabled here) yields component-specific payloads
//! (`ComponentImportSection`, `ModuleSection`, ...) for a WASM
//! *component*-encoded artifact — none of which match
//! `Payload::ImportSection`, so the original scan silently fell through
//! and returned `Ok(())` for a crafted component binary with smuggled
//! imports. `wasmtime::Module::new`/`from_binary` already categorically
//! reject the component encoding before compilation
//! (`wasmtime_environ::ModuleEnvironment::translate` bails with "expected
//! a WebAssembly module but was given a WebAssembly component" on the very
//! first payload), so this was never an exploitable bypass — but
//! [`verify_wasm_module`] now explicitly rejects `Encoding::Component`
//! itself, so the sanitizer's own `Ok(())` never silently no-ops on an
//! encoding it wasn't designed to scan.

use crate::types::AdmissionError;
use wasmparser::{Encoding, FuncType, Parser, Payload, TypeRef, ValType};

/// The host-import ABI a compiled guest module is permitted to use.
///
/// This is the SAME set `build_instance_pre` (in shamir-wasm-host's
/// `wasm_function.rs`) registers on the
/// [`wasmtime::Linker`] — kept in sync by a dedicated cross-check test
/// (in the host crate: `tests::wasm::admission::sanitizer_tests::sanctioned_list_matches_linker_registrations`)
/// rather than by having one generate the other, since `func_wrap`/
/// `func_wrap_async` need distinctly-typed Rust closures per import and
/// cannot be driven from a single generic loop over this array.
///
/// Every entry is `(module, name)`; the module is always `"shamir_host"`
/// today (no WASI or any other import namespace is ever registered).
pub const SANCTIONED_HOST_IMPORTS: &[(&str, &str)] = &[
    ("shamir_host", "batch_put"),
    ("shamir_host", "batch_get"),
    ("shamir_host", "global_set"),
    ("shamir_host", "global_get"),
    ("shamir_host", "call"),
    ("shamir_host", "call_checked"),
    ("shamir_host", "db_get"),
    ("shamir_host", "db_insert"),
    ("shamir_host", "db_query"),
    ("shamir_host", "db_execute"),
    ("shamir_host", "raise_error"),
    ("shamir_host", "http_fetch"),
];

fn check_checked_call_signature(type_index: u32, types: &[FuncType]) -> Result<(), AdmissionError> {
    let found = usize::try_from(type_index)
        .ok()
        .and_then(|index| types.get(index))
        .ok_or_else(|| AdmissionError::Validation("call_checked has unknown type index".into()))?;
    if found.params() == [ValType::I32; 6] && found.results() == [ValType::I64] {
        return Ok(());
    }
    let params = found
        .params()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let results = found
        .results()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Err(AdmissionError::WrongImportType {
        module: "shamir_host",
        name: crate::abi::IMPORT_CALL_CHECKED,
        expected: "(i32, i32, i32, i32, i32, i32) -> i64",
        found: format!("({params}) -> {results}"),
    })
}

/// Structurally verify a compiled `.wasm` module's import section against
/// [`SANCTIONED_HOST_IMPORTS`] — an allowlist posture (reject anything not
/// explicitly permitted, not a denylist of known-bad names).
///
/// Parses only the section headers via [`wasmparser::Parser`] (no
/// Cranelift compilation), so a malicious or malformed artifact is
/// rejected far more cheaply than by attempting full `wasmtime::Module`
/// construction. Every import must be a function import whose
/// `(module, name)` pair is in [`SANCTIONED_HOST_IMPORTS`]; an import
/// from any other module (a smuggled `env.*`/WASI-style import, or any
/// host-function name not on the sanctioned list) is rejected.
///
/// Returns `Ok(())` for a module with no imports, or whose imports are
/// all sanctioned. Returns `Err` on the first unsanctioned import found,
/// or on a `.wasm` byte stream that fails to parse structurally (the
/// caller's subsequent `wasmtime::Module::new`/`from_binary` call would
/// reject the same bytes anyway, just after paying for full validation).
pub fn verify_wasm_module(bytes: &[u8]) -> Result<(), AdmissionError> {
    let mut types = Vec::<FuncType>::new();
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| AdmissionError::Parse(e.to_string()))?;
        // Explicitly reject the WASM *component* encoding rather than
        // silently falling through the `ImportSection` match below.
        // `wasmparser`'s `component-model` feature (on by default, and
        // enabled transitively here) yields component-specific payloads
        // (`ComponentImportSection`, `ModuleSection`, ...) for a
        // component-format artifact — none of which match
        // `Payload::ImportSection`, so without this check a crafted
        // component binary with smuggled imports would silently pass
        // this scan as `Ok(())`. `wasmtime::Module::new`/`from_binary`
        // (called immediately after this sanitizer) already categorically
        // reject `Encoding::Component` before compilation, so this was
        // not an exploitable bypass in practice — but a sanitizer whose
        // whole purpose is being an explicit, auditable allowlist must
        // not silently no-op on an encoding it wasn't designed to parse.
        if let Payload::Version {
            encoding: Encoding::Component,
            ..
        } = payload
        {
            return Err(AdmissionError::ComponentEncoding);
        }
        if let Payload::TypeSection(reader) = &payload {
            for ty in reader.clone().into_iter_err_on_gc_types() {
                types.push(ty.map_err(|e| AdmissionError::Parse(e.to_string()))?);
            }
        }
        let Payload::ImportSection(reader) = payload else {
            continue;
        };
        for import in reader.into_imports() {
            let import =
                import.map_err(|e| AdmissionError::Parse(format!("malformed import: {e}")))?;
            // Only function imports carry the `shamir_host` ABI; a
            // memory/global/table/tag import under any name is not part
            // of the sanctioned surface either, so it is rejected the
            // same way as an unsanctioned function import below —
            // matching on `TypeRef` is not required here because the
            // allowlist check below is purely name-based and rejects
            // anything not in the list regardless of `import.ty`.
            let sanctioned = SANCTIONED_HOST_IMPORTS
                .iter()
                .any(|(m, n)| *m == import.module && *n == import.name);
            if !sanctioned {
                let kind = match import.ty {
                    TypeRef::Func(_) | TypeRef::FuncExact(_) => "func",
                    TypeRef::Table(_) => "table",
                    TypeRef::Memory(_) => "memory",
                    TypeRef::Global(_) => "global",
                    TypeRef::Tag(_) => "tag",
                };
                return Err(AdmissionError::UnsanctionedImport {
                    module: import.module.to_string(),
                    name: import.name.to_string(),
                    kind,
                });
            }
            if import.module == "shamir_host" && import.name == crate::abi::IMPORT_CALL_CHECKED {
                if let TypeRef::Func(type_index) | TypeRef::FuncExact(type_index) = import.ty {
                    check_checked_call_signature(type_index, &types)?;
                }
            }
        }
    }
    Ok(())
}

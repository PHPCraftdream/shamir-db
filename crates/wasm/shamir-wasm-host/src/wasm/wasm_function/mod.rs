//! [`WasmFunction`] — the [`ShamirFunction`] implementation backed by a
//! compiled Wasmtime [`Module`].
//!
//! A [`WasmFunction`] wraps a compiled Wasmtime [`Module`] and implements the
//! [`ShamirFunction`] trait. Execution uses Wasmtime's async support so host
//! imports can `.await` (e.g. `ctx.call` for function-calls-function).
//!
//! # Guest ABI
//!
//! The module **must** export:
//!
//! * `memory` — the linear memory.
//! * `shamir_alloc(len: i32) -> i32` — allocate `len` bytes in guest memory,
//!   return the starting pointer.
//! * `shamir_call(ptr: i32, len: i32) -> i64` — execute the function.
//!   Input msgpack bytes are at `[ptr, ptr+len)`. Returns a packed `i64`:
//!   `((out_ptr as i64) << 32) | (out_len as i64 & 0xFFFF_FFFF)` pointing at
//!   the msgpack-encoded result [`QueryValue`].
//!
//! # Host imports (slice 6)
//!
//! The host registers synchronous helper functions under the import module
//! `"shamir_host"` so the guest can access the batch context and global
//! variables:
//!
//! * `batch_put(key_ptr, key_len, val_ptr, val_len)` — write to batch.
//! * `batch_get(key_ptr, key_len) -> i64` — read from batch (0 = absent).
//! * `global_set(key_ptr, key_len, val_ptr, val_len)` — write a global var.
//! * `global_get(key_ptr, key_len) -> i64` — read a global var (0 = absent).
//! * `raise_error(kind, msg_ptr, msg_len)` — terminal error import; never returns successfully and maps guest error categories to host errors.
//!
//! # Host imports (slice 8a — async)
//!
//! * `call(name_ptr, name_len, params_ptr, params_len) -> i64` — invoke
//!   another registered function by name. Uses the same batch context and
//!   globals, with a depth limit to bound recursion.
//!
//! For the `*_get` variants and `call`, a non-zero return is
//! `((ptr as i64) << 32) | (len as i64)` where `[ptr, ptr+len)` in guest
//! memory holds the msgpack-encoded value. The host calls the guest's
//! `shamir_alloc` to create the buffer and writes into it before returning.
//!
//! Guests that do **not** import these symbols are unaffected (unused Linker
//! definitions are silently ignored by Wasmtime).
//!
//! # Async execution
//!
//! Since slice 8a the engine uses `Config::async_support(true)`. The four
//! original sync host imports remain sync (they are allowed under async_support
//! and never `.await`). The new `call` host import is async — it must await
//! `FunctionRegistry::invoke`.
//!
//! Guest execution is bounded three ways: per-Store FUEL (instruction count),
//! EPOCH interruption (wall-clock pre-emption of a pure-CPU guest that never
//! awaits — engine epoch ticker + per-Store epoch deadline), and a top-level
//! WALL-CLOCK deadline (`tokio::time::timeout`) bounding the TOTAL time of a
//! request across every nested `ctx.call`.
//!
//! Exclusive fuel-lease accounting (P0.1, invariant `B = F + ΣRᵢ + C`): the
//! shared counter threaded via `host_call.rs` is the FREE-capacity pool.
//! Each Store leases (reserves) its fuel from the pool BEFORE it becomes
//! executable; when a level suspends in a nested host call its executable
//! fuel is revoked and the unused capacity released so the child can lease
//! it; on settlement the Store refunds whatever remains unused. A suspended
//! ancestor retains no executable fuel, and the aggregate instruction count
//! across the whole `ctx.call` fan-out is bounded by one shared budget —
//! same as wall-clock + epoch already bound total time.
//!
//! # Two-tier compilation
//!
//! Creation compiles the fast tier (Winch); crossing `TierOptions::hot_calls`
//! swaps in an optimized-tier (Cranelift) slot; in-flight calls keep theirs.

mod call_exec;
mod function;
mod guest_io;
mod linker;
pub(crate) mod metrics;
mod state;

pub(crate) use function::run_compile_blocking;
pub use function::WasmFunction;
#[cfg(test)]
pub(crate) use guest_io::encode_params;
pub(in crate::wasm) use guest_io::{read_guest_mem, write_bytes_to_guest, write_value_to_guest};
pub(crate) use linker::build_linker;
#[cfg(test)]
pub(crate) use state::test_linker_and_store;
pub(crate) use state::HostState;

//! Placeholder execution-context handles (slice 3 → slice 6 → slice 8b).
//!
//! [`Ctx`] provides access to global variables (process-lifetime, shared
//! across batches), function calls, and database operations.
//! [`Batch`] provides access to the per-batch scratchpad
//! so functions in the same batch can exchange data.
//!
//! The real WASM host-import shims live in [`crate::host_imports`]; the
//! methods here delegate to them. On non-wasm targets the imports panic
//! (they can only be called from inside a WASM module).

/// Access to the DBMS on the current transaction.
///
/// Global variables are process-lifetime and shared across all batches.
/// The `new()` constructor creates a fresh context for backward compat on
/// the host target (the macro always calls `new()` at the top of
/// `shamir_call`; the real handles are injected by the host via the store
/// data, not by the guest constructor).
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    _private: (),
}

impl Ctx {
    /// Construct an empty context.
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Read a global variable. Returns `None` if absent.
    pub fn global_get(&self, key: &str) -> Option<crate::Value> {
        crate::host_imports::global_get(key)
    }

    /// Set a global variable.
    pub fn global_set(&self, key: &str, value: crate::Value) {
        crate::host_imports::global_set(key, value);
    }

    /// Invoke another registered function by name.
    ///
    /// `args` should be a `Value::Map` whose entries become the callee's
    /// `Params`. Returns the callee's result as a `Value`.
    ///
    /// If the callee is not found, the recursion depth limit is exceeded,
    /// or the callee itself errors, the host traps and the current function
    /// fails with a `Compute` error.
    pub fn call(&self, name: &str, args: crate::Value) -> crate::Value {
        crate::host_imports::call(name, args)
    }

    /// Obtain a database access handle for the default repo.
    ///
    /// Must be invoked via `invoke_function_in_db` (which sets up the
    /// gateway); traps if the host didn't provide one.
    ///
    /// ```ignore
    /// ctx.db().table("users").insert(doc)
    /// ctx.db().table("users").query(None)
    /// ```
    pub fn db(&self) -> crate::Db {
        crate::db::Db::new()
    }
}

/// The batch the function executes within.
///
/// Functions executing inside the same batch exchange intermediate values
/// through this scratchpad. The `new()` constructor creates a fresh batch
/// for backward compat on the host target.
#[derive(Debug, Clone, Default)]
pub struct Batch {
    _private: (),
}

impl Batch {
    /// Construct an empty batch view.
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Write a value into the batch scratchpad.
    pub fn put(&self, key: &str, value: crate::Value) {
        crate::host_imports::batch_put(key, value);
    }

    /// Read a value from the batch scratchpad. Returns `None` if absent.
    pub fn get(&self, key: &str) -> Option<crate::Value> {
        crate::host_imports::batch_get(key)
    }
}

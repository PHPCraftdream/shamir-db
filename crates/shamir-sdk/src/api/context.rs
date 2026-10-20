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

/// Execution context available to `#[procedure]` and `#[function]` kinds.
///
/// `Ctx` is the gateway to everything a non-pure function can do.
/// **Scalars (`#[scalar]`) intentionally have no `Ctx`** — that is the
/// purity guarantee.
///
/// # Reachable API
///
/// | Method | Returns | Purpose |
/// |--------|---------|---------|
/// | [`Ctx::db`] | [`Db`](crate::Db) | Database access (tables, queries) |
/// | [`Ctx::call`] | [`Value`](crate::Value) | Invoke another registered function |
/// | [`Ctx::http_fetch`] | [`Result<HttpResponse>`](crate::HttpResponse) | Egress HTTP via allowlist |
/// | [`Ctx::http_get`] | [`Result<HttpResponse>`](crate::HttpResponse) | Convenience GET wrapper |
/// | [`Ctx::http_post`] | [`Result<HttpResponse>`](crate::HttpResponse) | Convenience POST wrapper |
/// | [`Ctx::global_get`] | `Option<Value>` | Read a process-lifetime global variable |
/// | [`Ctx::global_set`] | `()` | Write a process-lifetime global variable |
///
/// # Examples
///
/// ```ignore
/// // Database: read all users
/// let users = ctx.db().table("users").query(None)?;
///
/// // Database: get one record by key
/// let rec = ctx.db().table("orders").get(Value::Int(42));
///
/// // Database: insert a document
/// ctx.db().table("logs").insert(Value::Map(vec![
///     ("event".into(), Value::Str("login".into())),
/// ]))?;
///
/// // Call another registered function
/// let doubled = ctx.call("double", Value::Map(vec![
///     ("n".into(), Value::Int(5)),
/// ]));
///
/// // HTTP egress (subject to allowlist + SSRF guard)
/// let resp = ctx.http_get("https://api.example.com/data")?;
/// let body = resp.body_text();
///
/// // Global variables (process-lifetime, shared across batches)
/// ctx.global_set("counter", Value::Int(1));
/// let v = ctx.global_get("counter"); // Some(Value::Int(1))
/// ```
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

    /// Send an HTTP request via the egress gateway (slice 8c).
    ///
    /// The host checks the request against the configured allowlist and
    /// SSRF guard before performing any network I/O. Traps only if egress
    /// is not configured at all (config bug). All runtime errors
    /// (allowlist denial, curl failure, timeout) are returned as
    /// catchable `Err`.
    ///
    /// # Wire envelope
    ///
    /// The host returns `Value::List([Bool, payload])`:
    /// - `[true, { status, headers, body }]` → `Ok(HttpResponse)`
    /// - `[false, "error message"]` → `Err(Error)`
    pub fn http_fetch(&self, req: crate::HttpRequest) -> crate::Result<crate::HttpResponse> {
        let raw = crate::host_imports::http_fetch(&req.to_value());
        crate::http::decode_fetch_envelope(&raw)
    }

    /// Convenience: HTTP GET the given URL and return the response.
    pub fn http_get(&self, url: &str) -> crate::Result<crate::HttpResponse> {
        self.http_fetch(crate::HttpRequest::get(url))
    }

    /// Convenience: HTTP POST the given body to the URL and return the response.
    pub fn http_post(&self, url: &str, body: Vec<u8>) -> crate::Result<crate::HttpResponse> {
        self.http_fetch(crate::HttpRequest::post(url, body))
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

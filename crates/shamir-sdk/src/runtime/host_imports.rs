//! Host-import ABI shims for accessing batch context, global variables,
//! function calls, and database operations.
//!
//! On `wasm32` these are thin `unsafe` wrappers around the functions
//! the host registers under the `"shamir_host"` import module. On all other
//! targets they panic because host imports are only available when running
//! inside the ShamirDB WASM runtime.
//!
//! # Database host imports (slice 8b)
//!
//! * `db_get(table_ptr, table_len, key_ptr, key_len) -> i64`
//! * `db_insert(table_ptr, table_len, doc_ptr, doc_len) -> i64`
//! * `db_query(table_ptr, table_len, filter_ptr, filter_len) -> i64`
//!
//! Table name is UTF-8 bytes; key/doc/filter are msgpack-encoded `Value`.
//! Zero-length filter means "no filter" (return all).
//! Return is packed `((ptr as i64) << 32) | (len as i64)` (0 = absent).

// ── WASM target: real imports ────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod imp {
    use crate::__rt::{decode_value_bytes, unpack_ptr_len, ResponseBytes};
    use crate::error::{Error, Result};
    use crate::Value;

    // The four host-import trampolines. Keys are passed as raw UTF-8 bytes;
    // values are passed as msgpack bytes. The `*_get` functions return
    // `((ptr as i64) << 32) | (len as i64)` (0 = absent).

    #[link(wasm_import_module = "shamir_host")]
    extern "C" {
        #[link_name = "batch_put"]
        fn host_batch_put(kp: i32, kl: i32, vp: i32, vl: i32);
        #[link_name = "batch_get"]
        fn host_batch_get(kp: i32, kl: i32) -> i64;
        #[link_name = "global_get"]
        fn host_global_get(kp: i32, kl: i32) -> i64;
        #[link_name = "global_set"]
        fn host_global_set(kp: i32, kl: i32, vp: i32, vl: i32);
        #[link_name = "call"]
        fn host_call(np: i32, nl: i32, pp: i32, pl: i32) -> i64;
        #[link_name = "db_get"]
        fn host_db_get(tp: i32, tl: i32, kp: i32, kl: i32) -> i64;
        #[link_name = "db_insert"]
        fn host_db_insert(tp: i32, tl: i32, dp: i32, dl: i32) -> i64;
        #[link_name = "db_query"]
        fn host_db_query(tp: i32, tl: i32, fp: i32, fl: i32) -> i64;
        #[link_name = "http_fetch"]
        fn host_http_fetch(rp: i32, rl: i32) -> i64;

        #[cfg(feature = "query-builder")]
        #[link_name = "db_execute"]
        fn host_db_execute(rp: i32, rl: i32) -> i64;
    }

    /// Encode `value` into an owned [`RequestBuffer`] for one host call.
    ///
    /// The host reads the bytes synchronously during the extern call, so the
    /// buffer drops normally at the end of the shim instead of leaking.
    fn with_request<T>(value: &Value, call: impl FnOnce(i32, i32) -> T) -> Result<T> {
        let buf = crate::__rt::RequestBuffer::encode(value)?;
        Ok(call(buf.ptr(), buf.len()))
    }

    /// Decode the host's msgpack response bytes. Fail-closed: a codec error
    /// carries its cause instead of becoming a plausible value. The response
    /// allocation (made via the guest's `shamir_alloc`) is owned by the
    /// [`ResponseBytes`] guard and freed on both the Ok and Err paths.
    ///
    /// # Safety
    /// The host must have written `len` msgpack bytes at `ptr` for this call.
    unsafe fn decode_response(ptr: i32, len: i32) -> Result<Value> {
        let guard = ResponseBytes::from_raw(ptr, len);
        decode_value_bytes(guard.as_slice())
    }

    pub fn batch_put(key: &str, value: Value) -> Result<()> {
        let kp = key.as_ptr() as i32;
        let kl = key.len() as i32;
        with_request(&value, |vp, vl| unsafe { host_batch_put(kp, kl, vp, vl) })
    }

    pub fn batch_get(key: &str) -> Result<Option<Value>> {
        let kp = key.as_ptr() as i32;
        let kl = key.len() as i32;
        // Safety: host reads key from guest memory synchronously.
        let packed = unsafe { host_batch_get(kp, kl) };
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Ok(None),
        };
        // Safety: the host wrote the msgpack bytes into guest memory via
        // shamir_alloc. The buffer remains valid for the rest of this call.
        Ok(Some(unsafe { decode_response(ptr, len)? }))
    }

    pub fn global_get(key: &str) -> Result<Option<Value>> {
        let kp = key.as_ptr() as i32;
        let kl = key.len() as i32;
        let packed = unsafe { host_global_get(kp, kl) };
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Ok(None),
        };
        // Safety: the host wrote the msgpack bytes into guest memory via
        // shamir_alloc. The buffer remains valid for the rest of this call.
        Ok(Some(unsafe { decode_response(ptr, len)? }))
    }

    pub fn global_set(key: &str, value: Value) -> Result<()> {
        let kp = key.as_ptr() as i32;
        let kl = key.len() as i32;
        with_request(&value, |vp, vl| unsafe { host_global_set(kp, kl, vp, vl) })
    }

    /// Invoke another registered function by name.
    ///
    /// `args` is a `Value::Map` whose entries become the callee's `Params`.
    /// Returns the callee's result as a `Value`. The host traps if the callee
    /// is not found, the depth limit is exceeded, or the callee errors.
    pub fn call(name: &str, args: Value) -> Result<Value> {
        let np = name.as_ptr() as i32;
        let nl = name.len() as i32;
        let packed = with_request(&args, |pp, pl| unsafe { host_call(np, nl, pp, pl) })?;
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Err(Error::user("call: host returned no result")),
        };
        // Safety: the host wrote the msgpack bytes into guest memory for this call.
        unsafe { decode_response(ptr, len) }
    }

    /// Read a record from a table by key.
    ///
    /// `table` is the table name. `key` is a `Value::Map` of primary-key
    /// fields (e.g. `{\"id\": 1}`) or a scalar (treated as filter on \"id\").
    /// Returns `Ok(None)` if no record matches.
    pub fn db_get(table: &str, key: &Value) -> Result<Option<Value>> {
        let tp = table.as_ptr() as i32;
        let tl = table.len() as i32;
        let packed = with_request(key, |kp, kl| unsafe { host_db_get(tp, tl, kp, kl) })?;
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Ok(None),
        };
        // Safety: the host wrote the msgpack bytes into guest memory for this call.
        Ok(Some(unsafe { decode_response(ptr, len)? }))
    }

    /// Insert a document into a table. Returns the stored record.
    ///
    /// `doc` must be a `Value::Map`. Traps on error.
    pub fn db_insert(table: &str, doc: &Value) -> Result<Value> {
        let tp = table.as_ptr() as i32;
        let tl = table.len() as i32;
        let packed = with_request(doc, |dp, dl| unsafe { host_db_insert(tp, tl, dp, dl) })?;
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Err(Error::user("db_insert: host returned no record")),
        };
        // Safety: the host wrote the msgpack bytes into guest memory for this call.
        unsafe { decode_response(ptr, len) }
    }

    /// Query records from a table with an optional filter.
    ///
    /// `filter` is `None` for "all records", or a `Value` that the host
    /// interprets as a filter (same key convention as `db_get`).
    /// Returns a `Value::List` of matching records.
    pub fn db_query(table: &str, filter: Option<&Value>) -> Result<Value> {
        let tp = table.as_ptr() as i32;
        let tl = table.len() as i32;
        let packed = match filter {
            Some(v) => with_request(v, |fp, fl| unsafe { host_db_query(tp, tl, fp, fl) })?,
            None => unsafe { host_db_query(tp, tl, core::ptr::null::<u8>() as i32, 0i32) },
        };
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Err(Error::user("db_query: host returned no result")),
        };
        // Safety: the host wrote the msgpack bytes into guest memory for this call.
        unsafe { decode_response(ptr, len) }
    }

    /// Send an HTTP request via the host gateway.
    ///
    /// `req` must be a `Value::Map` with the shape:
    /// ```text
    /// { "method": Str, "url": Str, "headers": Map, "body": Bin }
    /// ```
    ///
    /// Returns a `Value::List([Bool, payload])` envelope:
    /// - `[true, { "status": Int, "headers": Map, "body": Bin }]` on success.
    /// - `[false, "error message"]` on runtime error (allowlist denial,
    ///   curl failure, timeout).
    ///
    /// Traps only if egress is not configured at all (config bug).
    pub fn http_fetch(req: &Value) -> Result<Value> {
        let packed = with_request(req, |rp, rl| unsafe { host_http_fetch(rp, rl) })?;
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Err(Error::user("http_fetch: host returned no result")),
        };
        // Safety: the host wrote the msgpack bytes into guest memory for this call.
        unsafe { decode_response(ptr, len) }
    }

    /// Execute a full msgpack-encoded `BatchRequest`; returns the msgpack
    /// `BatchResponse` bytes (empty vec if the host returned nothing).
    #[cfg(feature = "query-builder")]
    pub fn db_execute(request: &[u8]) -> Vec<u8> {
        let rp = request.as_ptr() as i32;
        let rl = request.len() as i32;
        // Safety: host reads the request bytes from guest memory synchronously.
        let packed = unsafe { host_db_execute(rp, rl) };
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Vec::new(),
        };
        // Safety: the host wrote len response bytes at ptr into guest memory
        // via the guest's shamir_alloc for this call; the guard owns and
        // frees that allocation after the copy.
        let guard = unsafe { ResponseBytes::from_raw(ptr, len) };
        guard.as_slice().to_vec()
    }
}

// ── Non-WASM target: stubs that panic ────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use crate::error::Result;
    use crate::Value;

    fn host_only() -> ! {
        panic!("host imports are only available under wasm32")
    }

    pub fn batch_put(_key: &str, _value: Value) -> Result<()> {
        host_only()
    }

    pub fn batch_get(_key: &str) -> Result<Option<Value>> {
        host_only()
    }

    pub fn global_get(_key: &str) -> Result<Option<Value>> {
        host_only()
    }

    pub fn global_set(_key: &str, _value: Value) -> Result<()> {
        host_only()
    }

    /// Invoke another registered function by name (host-only).
    pub fn call(_name: &str, _args: Value) -> Result<Value> {
        host_only()
    }

    /// Read a record by key (host-only).
    pub fn db_get(_table: &str, _key: &Value) -> Result<Option<Value>> {
        host_only()
    }

    /// Insert a document (host-only).
    pub fn db_insert(_table: &str, _doc: &Value) -> Result<Value> {
        host_only()
    }

    /// Query records (host-only).
    pub fn db_query(_table: &str, _filter: Option<&Value>) -> Result<Value> {
        host_only()
    }

    /// HTTP egress: send a request and get a response (host-only).
    pub fn http_fetch(_req: &Value) -> Result<Value> {
        host_only()
    }

    /// Execute a full batch (host-only stub).
    #[cfg(feature = "query-builder")]
    pub fn db_execute(_request: &[u8]) -> Vec<u8> {
        host_only()
    }
}

pub(crate) use imp::*;

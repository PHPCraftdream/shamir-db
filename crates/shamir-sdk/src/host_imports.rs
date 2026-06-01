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
    }

    /// Encode `value` to msgpack, leak the bytes, and return `(ptr, len)`.
    ///
    /// Leaked buffers live in guest linear memory and remain valid for the
    /// duration of a single host-call (the host reads them synchronously;
    /// the Store is dropped after `shamir_call` returns).
    fn encode_leak(value: &Value) -> (i32, i32) {
        let bytes = crate::__rt::encode_value(value);
        let len = bytes.len() as i32;
        let ptr = bytes.as_ptr() as i32;
        core::mem::forget(bytes);
        (ptr, len)
    }

    /// Decode a packed `i64` return into `(ptr, len)` guest-memory range.
    /// `packed == 0` means absent.
    fn unpack_ptr_len(packed: i64) -> Option<(i32, i32)> {
        if packed == 0 {
            return None;
        }
        let ptr = (packed >> 32) as i32;
        let len = (packed & 0xFFFF_FFFF) as i32;
        Some((ptr, len))
    }

    pub fn batch_put(key: &str, value: Value) {
        let kp = key.as_ptr() as i32;
        let kl = key.len() as i32;
        let (vp, vl) = encode_leak(&value);
        // Safety: host reads from guest memory synchronously; buffers live
        // for the duration of the call.
        unsafe { host_batch_put(kp, kl, vp, vl) };
    }

    pub fn batch_get(key: &str) -> Option<Value> {
        let kp = key.as_ptr() as i32;
        let kl = key.len() as i32;
        // Safety: host reads key from guest memory synchronously.
        let packed = unsafe { host_batch_get(kp, kl) };
        let (ptr, len) = unpack_ptr_len(packed)?;
        // Safety: the host wrote the msgpack bytes into guest memory via
        // shamir_alloc. The buffer remains valid for the rest of this call.
        let bytes: &[u8] = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        rmp_serde::from_slice(bytes).ok()
    }

    pub fn global_get(key: &str) -> Option<Value> {
        let kp = key.as_ptr() as i32;
        let kl = key.len() as i32;
        let packed = unsafe { host_global_get(kp, kl) };
        let (ptr, len) = unpack_ptr_len(packed)?;
        let bytes: &[u8] = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        rmp_serde::from_slice(bytes).ok()
    }

    pub fn global_set(key: &str, value: Value) {
        let kp = key.as_ptr() as i32;
        let kl = key.len() as i32;
        let (vp, vl) = encode_leak(&value);
        unsafe { host_global_set(kp, kl, vp, vl) };
    }

    /// Invoke another registered function by name.
    ///
    /// `args` is a `Value::Map` whose entries become the callee's `Params`.
    /// Returns the callee's result as a `Value`. Traps if the callee is
    /// not found, depth limit is exceeded, or the callee errors.
    pub fn call(name: &str, args: Value) -> Value {
        let np = name.as_ptr() as i32;
        let nl = name.len() as i32;
        let (pp, pl) = encode_leak(&args);
        let packed = unsafe { host_call(np, nl, pp, pl) };
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Value::Null,
        };
        let bytes: &[u8] = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        rmp_serde::from_slice(bytes).unwrap_or(Value::Null)
    }

    /// Read a record from a table by key.
    ///
    /// `table` is the table name. `key` is a `Value::Map` of primary-key
    /// fields (e.g. `{\"id\": 1}`) or a scalar (treated as filter on \"id\").
    /// Returns `None` if no record matches.
    pub fn db_get(table: &str, key: &Value) -> Option<Value> {
        let tp = table.as_ptr() as i32;
        let tl = table.len() as i32;
        let (kp, kl) = encode_leak(key);
        let packed = unsafe { host_db_get(tp, tl, kp, kl) };
        let (ptr, len) = unpack_ptr_len(packed)?;
        let bytes: &[u8] = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        rmp_serde::from_slice(bytes).ok()
    }

    /// Insert a document into a table. Returns the stored record.
    ///
    /// `doc` must be a `Value::Map`. Traps on error.
    pub fn db_insert(table: &str, doc: &Value) -> Value {
        let tp = table.as_ptr() as i32;
        let tl = table.len() as i32;
        let (dp, dl) = encode_leak(doc);
        let packed = unsafe { host_db_insert(tp, tl, dp, dl) };
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Value::Null,
        };
        let bytes: &[u8] = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        rmp_serde::from_slice(bytes).unwrap_or(Value::Null)
    }

    /// Query records from a table with an optional filter.
    ///
    /// `filter` is `None` for "all records", or a `Value` that the host
    /// interprets as a filter (same key convention as `db_get`).
    /// Returns a `Value::List` of matching records.
    pub fn db_query(table: &str, filter: Option<&Value>) -> Value {
        let tp = table.as_ptr() as i32;
        let tl = table.len() as i32;
        let (fp, fl) = match filter {
            Some(v) => encode_leak(v),
            None => (core::ptr::null::<u8>() as i32, 0i32),
        };
        let packed = unsafe { host_db_query(tp, tl, fp, fl) };
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Value::List(Vec::new()),
        };
        let bytes: &[u8] = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        rmp_serde::from_slice(bytes).unwrap_or(Value::List(Vec::new()))
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
    pub fn http_fetch(req: &Value) -> Value {
        let (rp, rl) = encode_leak(req);
        let packed = unsafe { host_http_fetch(rp, rl) };
        let (ptr, len) = match unpack_ptr_len(packed) {
            Some(pair) => pair,
            None => return Value::Null,
        };
        let bytes: &[u8] = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        rmp_serde::from_slice(bytes).unwrap_or(Value::Null)
    }
}

// ── Non-WASM target: stubs that panic ────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use crate::Value;

    fn host_only() -> ! {
        panic!("host imports are only available under wasm32")
    }

    pub fn batch_put(_key: &str, _value: Value) {
        host_only()
    }

    pub fn batch_get(_key: &str) -> Option<Value> {
        host_only()
    }

    pub fn global_get(_key: &str) -> Option<Value> {
        host_only()
    }

    pub fn global_set(_key: &str, _value: Value) {
        host_only()
    }

    /// Invoke another registered function by name (host-only).
    pub fn call(_name: &str, _args: Value) -> Value {
        host_only()
    }

    /// Read a record by key (host-only).
    pub fn db_get(_table: &str, _key: &Value) -> Option<Value> {
        host_only()
    }

    /// Insert a document (host-only).
    pub fn db_insert(_table: &str, _doc: &Value) -> Value {
        host_only()
    }

    /// Query records (host-only).
    pub fn db_query(_table: &str, _filter: Option<&Value>) -> Value {
        host_only()
    }

    /// HTTP egress: send a request and get a response (host-only).
    pub fn http_fetch(_req: &Value) -> Value {
        host_only()
    }
}

pub(crate) use imp::*;

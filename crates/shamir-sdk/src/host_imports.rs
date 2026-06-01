//! Host-import ABI shims for accessing batch context and global variables.
//!
//! On `wasm32` these are thin `unsafe` wrappers around the four functions
//! the host registers under the `"shamir_host"` import module. On all other
//! targets they panic because host imports are only available when running
//! inside the ShamirDB WASM runtime.

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
}

pub(crate) use imp::*;

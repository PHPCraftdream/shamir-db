//! Hidden runtime helpers used by code generated from `#[shamir_sdk_macros::function]`.
//!
//! Nothing in this module is part of the public SDK surface.
//!
//! Interoperable exchange-buffer pointers and lengths are nonnegative signed
//! i32 fields. Their byte ranges must fit in guest linear memory and lie in
//! the lower 2 GiB. The host's final packed-result reader extracts unsigned
//! fields; that does not relax the signed contract for SDK exchange buffers.
//! Bounds checks do not prove allocation provenance or ownership: host replies
//! must still refer to live, uniquely transferred `shamir_alloc` allocations.

use crate::error::{Error, Result};
use crate::params::Params;
use crate::value::Value;
use core::future::Future;
#[cfg(test)]
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

/// Decode msgpack bytes into a `Params` (the host sends a string-keyed map).
///
/// Malformed bytes or a non-map payload fail closed: no silent empty `Params`.
pub fn decode_params(bytes: &[u8]) -> Result<Params> {
    match rmp_serde::from_slice::<Value>(bytes) {
        Ok(Value::Map(entries)) => Ok(Params::from_map(entries)),
        Ok(_) => Err(Error::codec("decode params: expected a string-keyed map")),
        Err(e) => Err(Error::codec_with_source(format!("decode params: {e}"), e)),
    }
}

/// Encode a `Value` to msgpack bytes. Codec failures surface as errors
/// instead of an empty-byte fallback.
pub fn encode_value(value: &Value) -> Result<Vec<u8>> {
    rmp_serde::to_vec(value).map_err(|e| Error::codec_with_source(format!("encode value: {e}"), e))
}

/// Convert an exchange buffer to signed ABI fields after checking its range.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn guest_buffer_fields(ptr: usize, len: usize) -> Result<(i32, i32)> {
    let ptr = i32::try_from(ptr).map_err(|_| Error::host("guest buffer pointer out of range"))?;
    let len = i32::try_from(len).map_err(|_| Error::host("guest buffer length out of range"))?;
    validate_response_range(ptr, len, 1u64 << 31)?;
    Ok((ptr, len))
}

/// An owned request buffer handed to a host import.
///
/// The host reads the bytes synchronously during the extern call, so the
/// buffer stays owned and drops normally at the end of the shim — no
/// `mem::forget` of request payloads.
pub struct RequestBuffer {
    bytes: Vec<u8>,
}

#[cfg(test)]
static LIVE_REQUEST_BUFFERS: AtomicUsize = AtomicUsize::new(0);

impl RequestBuffer {
    /// Encode `value` into an owned request buffer.
    pub fn encode(value: &Value) -> Result<Self> {
        let bytes = encode_value(value)?;
        #[cfg(target_arch = "wasm32")]
        guest_buffer_fields(bytes.as_ptr() as usize, bytes.len())?;
        #[cfg(test)]
        LIVE_REQUEST_BUFFERS.fetch_add(1, Ordering::SeqCst);
        Ok(Self { bytes })
    }

    /// Pointer to the first byte, for the `(ptr, len)` host-import ABI.
    #[cfg(target_arch = "wasm32")]
    pub fn ptr(&self) -> i32 {
        guest_buffer_fields(self.bytes.as_ptr() as usize, self.bytes.len())
            .unwrap_or_else(|error| raise(&error))
            .0
    }

    /// Byte length, for the `(ptr, len)` host-import ABI.
    pub fn len(&self) -> i32 {
        i32::try_from(self.bytes.len())
            .unwrap_or_else(|_| trap("request length outside guest ABI range"))
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl Drop for RequestBuffer {
    fn drop(&mut self) {
        #[cfg(test)]
        LIVE_REQUEST_BUFFERS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Test-only count of currently live [`RequestBuffer`]s (leak oracle).
#[cfg(test)]
pub(crate) fn live_request_buffers() -> usize {
    LIVE_REQUEST_BUFFERS.load(Ordering::SeqCst)
}

/// Leak the final result bytes and return the packed `(ptr << 32) | len`
/// that the host reads back after `shamir_call` returns.
///
/// # Ownership
/// The guest cannot free this buffer: the host reads it only after the
/// export returns, and the ABI has no guest-visible free export. The host
/// owns the leak: in `shamir-wasm-host` (`wasm/wasm_function.rs`) every
/// `ShamirFunction::call` builds a fresh `Store<HostState>` and wraps it in
/// an `AccountedStore` whose `Drop` (running on success, error, and future
/// cancellation) drops the `Store` — which drops the entire guest linear
/// memory — after the result bytes have been copied out. The leak is thus
/// bounded per invocation by the host-side instance/Store teardown.
#[cfg(target_arch = "wasm32")]
pub fn leak_result(bytes: Vec<u8>) -> i64 {
    let ptr = bytes.as_ptr() as usize;
    let len = bytes.len();
    if ptr > i32::MAX as usize
        || len > i32::MAX as usize
        || (ptr as u64 + len as u64) > (1u64 << 31)
    {
        trap("result outside guest ABI range");
    }
    core::mem::forget(bytes);
    (((ptr as u64) << 32) | len as u64) as i64
}

/// Check signed ABI fields and the complete range before reconstructing
/// ownership. This cannot establish allocation provenance or uniqueness.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn validate_response_range(ptr: i32, len: i32, memory_bytes: u64) -> Result<()> {
    if ptr < 0 || len < 0 || (ptr == 0 && len > 0) {
        return Err(Error::host("invalid host response pointer or length"));
    }
    let end = ptr as u64 + len as u64;
    if end > (1u64 << 31) || end > memory_bytes {
        return Err(Error::host("host response range outside guest memory"));
    }
    Ok(())
}

/// Owned view of a host-import response buffer.
///
/// The host writes import responses into guest memory via the guest's
/// generated `shamir_alloc`, which allocates `vec![0u8; len]` and forgets
/// it — so capacity equals length exactly and the allocation can be
/// reconstructed. This guard exposes the bytes while alive and frees the
/// allocation on `Drop` (skipped for `len == 0`: `shamir_alloc(0)` returns
/// a dangling-but-aligned pointer that must not be freed).
pub struct ResponseBytes {
    /// Native-width (`usize`) address of the allocation, so native test
    /// code can hand over a real pointer without truncating it to `i32`.
    ptr: usize,
    len: i32,
}

#[cfg(test)]
static LIVE_RESPONSE_BYTES: AtomicUsize = AtomicUsize::new(0);

impl ResponseBytes {
    /// Take ownership of a `shamir_alloc`-produced `(ptr, len)` range.
    ///
    /// # Safety
    /// For a nonempty response, `ptr..ptr+len` must be a live, initialized,
    /// uniquely owned allocation made by guest `shamir_alloc` with exactly
    /// `len` capacity, and must not be freed elsewhere. Numerical range checks
    /// do not establish these ownership and provenance requirements.
    #[cfg(target_arch = "wasm32")]
    pub unsafe fn from_raw(ptr: i32, len: i32) -> Result<Self> {
        let memory_bytes = core::arch::wasm32::memory_size(0) as u64 * 65536;
        validate_response_range(ptr, len, memory_bytes)?;
        #[cfg(test)]
        if len > 0 {
            LIVE_RESPONSE_BYTES.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Self {
            ptr: ptr as usize,
            len,
        })
    }

    /// Test-only constructor from a native pointer (no `i32` truncation).
    ///
    /// # Safety
    /// `ptr..ptr+len` must be a live, uniquely owned allocation made with
    /// `Vec<u8>` capacity exactly `len`, `len <= i32::MAX`, and not freed
    /// elsewhere.
    #[cfg(test)]
    pub(crate) unsafe fn from_native(ptr: *mut u8, len: usize) -> Self {
        #[cfg(test)]
        if len > 0 {
            LIVE_RESPONSE_BYTES.fetch_add(1, Ordering::SeqCst);
        }
        Self {
            ptr: ptr as usize,
            len: len as i32,
        }
    }

    /// The response bytes, valid only while this guard is alive.
    pub fn as_slice(&self) -> &[u8] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: the constructors require a live, initialized allocation;
        // it remains uniquely owned by this guard and is not mutated here.
        unsafe { core::slice::from_raw_parts(self.ptr as *const u8, self.len as usize) }
    }
}

impl Drop for ResponseBytes {
    fn drop(&mut self) {
        if self.len > 0 {
            // SAFETY: the constructor requires a live, uniquely transferred
            // Vec allocation with capacity and initialized length both equal to len.
            drop(unsafe {
                Vec::from_raw_parts(self.ptr as *mut u8, self.len as usize, self.len as usize)
            });
            #[cfg(test)]
            LIVE_RESPONSE_BYTES.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Test-only count of currently live non-empty [`ResponseBytes`] guards.
#[cfg(test)]
pub(crate) fn live_response_bytes() -> usize {
    LIVE_RESPONSE_BYTES.load(Ordering::SeqCst)
}

/// Pure core of host-import response decoding: fail closed with the codec
/// cause instead of a plausible fallback value.
pub fn decode_value_bytes(bytes: &[u8]) -> Result<Value> {
    rmp_serde::from_slice(bytes)
        .map_err(|e| Error::codec_with_source(format!("decode host response: {e}"), e))
}

/// Decode a packed `i64` host-import return into `(ptr, len)`.
/// `packed == 0` means absent (callers map `None` to "no value").
pub fn unpack_ptr_len(packed: i64) -> Option<(i32, i32)> {
    if packed == 0 {
        return None;
    }
    let ptr = (packed >> 32) as i32;
    let len = (packed & 0xFFFF_FFFF) as i32;
    Some((ptr, len))
}

/// Poll a future once on a no-op waker.
///
/// Wrapper futures across all guest function kinds are polled exactly once on
/// a no-op waker. A genuinely `Pending` future is an explicit error — the guest
/// ABI has no suspension protocol, so there is no re-poll loop.
pub fn block_on<F: Future>(future: F) -> Result<F::Output> {
    // Safety: a no-op waker that does nothing on wake/clone/drop.
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(core::ptr::null(), &VTABLE), // clone
        |_| {},                                        // wake
        |_| {},                                        // wake_by_ref
        |_| {},                                        // drop
    );
    let raw = RawWaker::new(core::ptr::null(), &VTABLE);
    // Safety: the vtable functions are valid and do not dereference the data pointer.
    let waker = unsafe { Waker::from_raw(raw) };
    let mut cx = Context::from_waker(&waker);

    let mut future = core::pin::pin!(future);
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(out) => Ok(out),
        Poll::Pending => Err(Error::host(
            "guest future returned Pending; suspension is not supported in guest ABI",
        )),
    }
}

/// Report a deliberate guest error through the terminal host import.
pub fn raise(error: &Error) -> ! {
    let message = sanitize_trap_message(error.message());
    #[cfg(target_arch = "wasm32")]
    let (ptr, len) = match guest_buffer_fields(message.as_ptr() as usize, message.len()) {
        Ok(fields) => fields,
        Err(_) => core::arch::wasm32::unreachable(),
    };
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: `message` remains alive and readable for the synchronous host import.
        unsafe {
            crate::runtime::host_imports::raise_error(error.kind().abi_code(), ptr, len);
        }
        core::arch::wasm32::unreachable();
    }
    #[cfg(not(target_arch = "wasm32"))]
    panic!("shamir function error: {:?}: {message}", error.kind());
}

/// Abort with a WASM trap. The host maps a trap to `FunctionError::Compute`.
pub fn trap(msg: &str) -> ! {
    let sanitized = sanitize_trap_message(msg);
    // The guest SDK is built with std available, so panic is fine.
    panic!("shamir function error: {sanitized}");
}

const MAX_TRAP_MESSAGE_BYTES: usize = 1024;

pub(crate) fn sanitize_trap_message(msg: &str) -> String {
    let mut sanitized = String::with_capacity(msg.len().min(MAX_TRAP_MESSAGE_BYTES));
    for ch in msg.chars() {
        let ch = if ch.is_control() { '\u{fffd}' } else { ch };
        if sanitized.len() + ch.len_utf8() > MAX_TRAP_MESSAGE_BYTES {
            break;
        }
        sanitized.push(ch);
    }
    sanitized
}

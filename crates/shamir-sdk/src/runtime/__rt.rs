//! Hidden runtime helpers used by code generated from `#[shamir_sdk_macros::function]`.
//!
//! Nothing in this module is part of the public SDK surface.

use crate::params::Params;
use crate::value::Value;
use core::future::Future;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

/// Decode msgpack bytes into a `Params` (the host sends a string-keyed map).
pub fn decode_params(bytes: &[u8]) -> Params {
    match rmp_serde::from_slice::<Value>(bytes) {
        Ok(Value::Map(entries)) => Params::from_map(entries),
        _ => Params::new(),
    }
}

/// Encode a `Value` to msgpack bytes.
pub fn encode_value(value: &Value) -> Vec<u8> {
    rmp_serde::to_vec(value).unwrap_or_else(|_| Vec::new())
}

/// Leak a `Vec<u8>` and return the packed `(ptr << 32) | len` that the host
/// reads back.
pub fn leak_result(bytes: Vec<u8>) -> i64 {
    let len = bytes.len() as u64;
    let ptr = bytes.as_ptr() as usize as u64;
    core::mem::forget(bytes);
    ((ptr << 32) | (len & 0xFFFF_FFFF)) as i64
}

/// Drive a future to completion on a no-op waker.
///
/// Works because pure functions (the only kind this slice supports) are
/// `Ready` on the first poll.
pub fn block_on<F: Future>(future: F) -> F::Output {
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
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(out) => return out,
            Poll::Pending => {
                // Pure functions never yield Pending. If a future genuinely
                // needs async I/O (slice 4 host imports), this will spin.
                // For now, a tight loop is correct.
                core::hint::spin_loop();
            }
        }
    }
}

/// Abort with a WASM trap. The host maps a trap to `FunctionError::Compute`.
pub fn trap(msg: &str) -> ! {
    // We can't format a nice message in no_std WASM without alloc, but
    // panic! is available with the standard `std` or `alloc` crate. The
    // guest SDK is built with std available, so panic is fine.
    panic!("shamir function error: {msg}");
}

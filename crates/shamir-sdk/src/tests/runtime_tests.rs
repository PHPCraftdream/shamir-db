//! Regression tests for the guest runtime ABI (`__rt`): fail-closed codec
//! handling, request-buffer ownership, and the guest executor poll contract.

use crate::__rt;
use crate::error::ErrorKind;
use crate::Value;
use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::rc::Rc;

#[test]
fn rv_sdk_6d2497ca_abi_codes_pinned() {
    assert_eq!(ErrorKind::User.abi_code(), 1);
    assert_eq!(ErrorKind::InvalidArgument.abi_code(), 2);
    assert_eq!(ErrorKind::Codec.abi_code(), 3);
    assert_eq!(ErrorKind::Host.abi_code(), 4);
}

/// RV-SDK-MACROS-D7F6AEC5: trap messages replace control characters and are
/// capped at 1024 bytes without splitting a UTF-8 code point.
#[test]
fn rv_sdk_macros_d7f6aec5_trap_message_sanitizer_bounds_utf8_and_controls() {
    let sanitized = __rt::sanitize_trap_message("line\n\tbreak\u{7f}");
    assert_eq!(sanitized, "line\u{fffd}\u{fffd}break\u{fffd}");

    let exact = format!("{}é", "a".repeat(1022));
    let sanitized = __rt::sanitize_trap_message(&exact);
    assert_eq!(sanitized.len(), 1024);
    assert!(sanitized.is_char_boundary(sanitized.len()));

    let oversized = format!("{}é", "a".repeat(1023));
    let sanitized = __rt::sanitize_trap_message(&oversized);
    assert_eq!(sanitized.len(), 1023);
    assert!(sanitized.is_char_boundary(sanitized.len()));
    assert!(!sanitized.chars().any(char::is_control));
}

#[test]
fn rv_sdk_macros_d7f6aec5_trap_panics_with_sanitized_bounded_payload() {
    let message = format!("line\n{}\tend", "x".repeat(1020));
    let panic = std::panic::catch_unwind(|| __rt::trap(&message)).expect_err("trap must panic");
    let payload = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&'static str>().copied())
        .expect("panic payload is text");
    let expected = format!("shamir function error: line\u{fffd}{}", "x".repeat(1017));
    assert_eq!(payload, expected);
}

/// Future that yields `Pending` for the first `pending_polls` polls, then
/// `Ready(Value::Int(7))`. Counts every poll into a shared cell.
struct CountingFuture {
    pending_polls: u32,
    polls: Rc<Cell<u32>>,
}

impl Future for CountingFuture {
    type Output = Value;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.polls.set(this.polls.get() + 1);
        if this.polls.get() <= this.pending_polls {
            Poll::Pending
        } else {
            Poll::Ready(Value::Int(7))
        }
    }
}

/// RV-SDK-8190492D: host-import request buffers must be owned and dropped,
/// not `mem::forget`ed. The test-only live counter observes exactly that.
#[test]
fn rv_sdk_8190492d_request_buffer_drops_after_use() {
    assert_eq!(__rt::live_request_buffers(), 0);
    {
        let buf = __rt::RequestBuffer::encode(&Value::Int(1)).expect("encode");
        assert_eq!(__rt::live_request_buffers(), 1);
        assert_eq!(buf.len(), 1); // msgpack positive fixint = 1 byte
        assert!(!buf.is_empty());
        #[cfg(target_arch = "wasm32")]
        assert!(buf.ptr() != 0);
    }
    assert_eq!(__rt::live_request_buffers(), 0);
}

/// RV-SDK-A68C70DE: malformed response/param bytes must fail closed with the
/// codec cause, not silently become empty `Params`.
#[test]
fn rv_sdk_a68c70de_decode_params_rejects_malformed_bytes() {
    // Truncated msgpack: map-of-1 header with a truncated key.
    let err = __rt::decode_params(&[0x81, 0xa1]).expect_err("malformed bytes must be rejected");
    assert!(err.message().starts_with("decode params:"));
}

/// RV-SDK-MACROS-4DB0D871: a valid non-map payload must be rejected (so the
/// validator no longer coerces it into record=Null / old_record=None), while
/// an empty map stays valid (legitimate empty parameters).
#[test]
fn rv_sdk_macros_4db0d871_decode_params_rejects_non_map_but_accepts_empty_map() {
    let int_bytes = rmp_serde::to_vec(&Value::Int(1)).unwrap();
    let err = __rt::decode_params(&int_bytes).expect_err("non-map payload must be rejected");
    assert!(err.message().contains("map"));

    let empty_map = rmp_serde::to_vec(&Value::Map(Vec::new())).unwrap();
    let params = __rt::decode_params(&empty_map).expect("empty map is valid params");
    assert_eq!(params.raw().len(), 0);
}

/// RV-SDK-MACROS-FEC0E5E4: a genuinely Pending guest future must become an
/// explicit error, not spin forever on a no-op waker.
#[test]
fn rv_sdk_macros_fec0e5e4_pending_future_is_explicit_error() {
    let polls = Rc::new(Cell::new(0));
    let err = __rt::block_on(CountingFuture {
        pending_polls: 1,
        polls: polls.clone(),
    })
    .expect_err("Pending future must be an explicit error");
    assert!(err.message().contains("Pending"));
    assert!(err.message().contains("suspension is not supported"));
}

/// RV-SDK-MACROS-D44EF500: controlled poll-count oracle for the
/// "Ready on first poll" contract — a Ready future is polled exactly once,
/// and a Pending future is polled exactly once before the explicit error
/// (no re-poll loop).
#[test]
fn rv_sdk_macros_d44ef500_poll_count_oracle_ready_on_first_poll() {
    let polls = Rc::new(Cell::new(0));
    let out = __rt::block_on(CountingFuture {
        pending_polls: 0,
        polls: polls.clone(),
    })
    .expect("ready future completes");
    assert_eq!(out, Value::Int(7));
    assert_eq!(polls.get(), 1, "Ready future must be polled exactly once");

    let pending_polls = Rc::new(Cell::new(0));
    assert!(__rt::block_on(CountingFuture {
        pending_polls: 1,
        polls: pending_polls.clone(),
    })
    .is_err());
    assert_eq!(
        pending_polls.get(),
        1,
        "Pending future must not be re-polled"
    );
}

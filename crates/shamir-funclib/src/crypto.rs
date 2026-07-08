//! `/crypto` scalar category — pure, deterministic crypto primitives.
//!
//! Functions registered (plain names, no folder prefix):
//! `sha256 sha512 sha3_256 blake3 hmac_sha256 ct_eq argon2id`.
//!
//! Conventions (mirroring `math.rs`):
//! - Hashes take a single `Bin` argument via [`arg_bytes`] and return the raw
//!   digest as `Bin` via [`v_bytes`].
//! - `hmac_sha256(key, msg)` takes two `Bin` arguments (key, message) and
//!   returns the 32-byte MAC as `Bin`.
//! - `ct_eq(a, b)` compares two `Bin` arguments in constant time (via `subtle`)
//!   and returns a `Bool`.
//! - `argon2id(password, salt, [memory_kb, time, parallelism, length])` is the
//!   Argon2id KDF. It is **deterministic** given its inputs (same password +
//!   salt + params → same digest), so it fits the pure-scalar contract — but
//!   it is **CPU- and memory-bound** (tens of ms at OWASP defaults). A caller
//!   dispatching it on an async runtime MUST offload to `spawn_blocking`; do
//!   not invoke it inline on a runtime worker.
//! - Every function here is `pure + deterministic` (no randomness, no clock),
//!   so all use [`FnEntry::pure`]. Non-deterministic procedural crypto
//!   (random / uuid) and asymmetric / PQC primitives remain out of scope.
//!
//! # `argon2id` aggregate concurrency cap (audit §2b)
//!
//! `argon2id()` is guest/query-reachable: any user or WASM guest function can
//! call it from a filter/validator/computed-default expression. Without an
//! aggregate cap on the *number* of concurrent invocations, a low-privileged
//! caller can exhaust server memory by issuing many parallel `argon2id()`
//! calls (each up to [`A2_MAX_MEMORY_KB`]). The process-wide
//! [`ARGON2ID_CONCURRENCY_GATE`] (a counting semaphore, capacity
//! [`ARGON2ID_CONCURRENCY_CAP`]) bounds the in-flight `hash_password_into`
//! calls across ALL connections/queries. See the note on [`argon2id_fn`] for
//! the inline-dispatch tension this gate cannot resolve on its own.

use crate::registry::{arg_bytes, arg_i64, v_bool, v_bytes, FnEntry, ScalarError, ScalarRegistry};
use argon2::{Algorithm, Argon2, Params as Argon2Params, Version};
use hmac::{Mac, SimpleHmac};
use sha2::{Digest, Sha256, Sha512};
use sha3::Sha3_256;
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Condvar, LazyLock, Mutex};
use subtle::ConstantTimeEq;

// Argon2id parameter defaults (OWASP interactive-login profile) and upper
// bounds (resource-exhaustion guard on untrusted input). Mirrors the engine's
// async `Argon2idFunction` so both paths agree bit-for-bit.
const A2_DEFAULT_MEMORY_KB: u32 = 19_456;
const A2_DEFAULT_TIME: u32 = 2;
const A2_DEFAULT_PARALLELISM: u32 = 1;
const A2_DEFAULT_LENGTH: u32 = 32;
// Per-call memory cap. Lowered from 1 GiB (1_048_576 KiB) to 64 MiB as
// secondary hardening per audit §2b: the OWASP interactive profile is 19 MiB
// and the OWASP "high-security" profile tops out around 64 MiB; values above
// that have no legitimate KDF use case from a *query-reachable* scalar (this
// is NOT the auth-hash path). The primary OOM defence is the aggregate
// concurrency cap below; this per-call ceiling tightens the worst-case
// single-call allocation so a single malicious call cannot pin 1 GiB.
const A2_MAX_MEMORY_KB: u32 = 65_536; // 64 MiB
const A2_MAX_TIME: u32 = 16;
const A2_MAX_PARALLELISM: u32 = 16;
const A2_MAX_LENGTH: u32 = 256;

/// Aggregate concurrency cap for the query-reachable `argon2id()` scalar
/// (audit §2b).
///
/// Bounds the number of `argon2id()` calls executing their `hash_password_into`
/// work *concurrently* across ALL connections/queries/guest functions. Chosen
/// **lower** than the auth path's 64 permits (`shamir-connect`'s
/// `Argon2Semaphore`) because this path additionally permits a larger per-call
/// memory profile (up to [`A2_MAX_MEMORY_KB`]):
/// - At the OWASP interactive default (19 MiB/call): 16 × 19 MiB ≈ 304 MiB.
/// - At the per-call ceiling (64 MiB): 16 × 64 MiB = 1 GiB aggregate worst-case.
///
/// Exposed as a `pub const` so tests/operators can observe and assert it.
pub const ARGON2ID_CONCURRENCY_CAP: u32 = 16;

/// Test/debug observability: peak number of `argon2id()` KDF calls that were
/// simultaneously inside the gated `hash_password_into` region since process
/// start. Updated by [`argon2id_fn`] via [`A2_IN_FLIGHT`] on each entry/exit.
/// Pure observation — does not gate or alter any production behaviour. Kept
/// `pub(crate)` so the concurrency-cap regression test can assert the cap
/// holds without test-only code on the hot path (the counter is two relaxed
/// atomic ops, negligible vs. the KDF itself).
pub(crate) static A2_PEAK_IN_FLIGHT: AtomicU32 = AtomicU32::new(0);
/// Current in-flight `argon2id()` KDF count (entry-inc / exit-dec around the
/// gated region). Paired with [`A2_PEAK_IN_FLIGHT`] for observability.
/// `pub(crate)` so the regression test can reset it between runs.
pub(crate) static A2_IN_FLIGHT: AtomicU32 = AtomicU32::new(0);

/// Process-wide counting semaphore gating `argon2id()`'s expensive KDF work.
///
/// Mirrors `shamir_connect::server::argon2_semaphore::Argon2Semaphore`'s design
/// (atomics + condvar, blocking acquire) but lives here in `shamir-funclib` to
/// avoid introducing a new inter-crate dependency edge (`shamir-funclib` does
/// not depend on `shamir-connect`, and the semaphore needs no crate-external
/// types). Held in a [`LazyLock`] process-global so it is shared across every
/// `ScalarRegistry` instance and every query engine without threading state
/// through `FnEntry` (which is a pure `fn(&[QueryValue]) -> ScalarResult`
/// contract with no side-channel for shared limiters).
///
/// **Inlining tension (audit §2b, residual risk):** the current engine
/// dispatches registered scalars *inline* on the async runtime worker — no
/// `spawn_blocking` wraps `scalars.call(...)` in `filter/resolve.rs`,
/// `table/write_helpers.rs`, or `validator/schema/field_rule.rs`. A blocking
/// `acquire()` here therefore stalls the runtime worker the same way the
/// uncapped Argon2 call already does today. The cap nonetheless bounds
/// aggregate Argon2 memory (the primary audit concern) and is the correct
/// minimal fix; moving scalar dispatch onto `spawn_blocking` project-wide is a
/// larger refactor flagged as follow-up.
static ARGON2ID_CONCURRENCY_GATE: LazyLock<CountingSemaphore> =
    LazyLock::new(|| CountingSemaphore::with_capacity(ARGON2ID_CONCURRENCY_CAP));

/// Minimal counting semaphore (atomics + condvar), mirroring `shamir-connect`'s
/// `Argon2Semaphore` so the two paths stay consistent. Kept private — the only
/// consumer is [`ARGON2ID_CONCURRENCY_GATE`].
struct CountingSemaphore {
    available: AtomicI64,
    notify: (Mutex<()>, Condvar),
}

impl CountingSemaphore {
    fn with_capacity(capacity: u32) -> Self {
        Self {
            available: AtomicI64::new(capacity as i64),
            notify: (Mutex::new(()), Condvar::new()),
        }
    }

    /// Block until a permit is available, then take one. Mirrors
    /// `Argon2Semaphore::acquire` (indefinite blocking wait).
    fn acquire(&self) {
        // Fast path: uncontended single CAS.
        if try_take(&self.available) {
            return;
        }
        // Contended path: park on the condvar until a release wakes us.
        let (lock, cvar) = &self.notify;
        let mut guard = lock.lock().expect("semaphore mutex poisoned");
        while !try_take(&self.available) {
            guard = cvar.wait(guard).expect("semaphore condvar poisoned");
        }
    }

    fn release(&self) {
        self.available.fetch_add(1, Ordering::Release);
        self.notify.1.notify_one();
    }
}

/// Decrement `available` by one if it is positive. Returns `true` if a permit
/// was taken. Used by both the fast and contended paths of [`CountingSemaphore::acquire`].
fn try_take(available: &AtomicI64) -> bool {
    available
        .fetch_update(Ordering::Acquire, Ordering::Relaxed, |v| {
            if v > 0 {
                Some(v - 1)
            } else {
                None
            }
        })
        .is_ok()
}

/// RAII permit — releases on drop. Held across `hash_password_into` only.
/// Construction blocks in [`CountingSemaphore::acquire`] until a permit is
/// available; [`Drop`] releases it.
struct SemaphorePermit<'a>(&'a CountingSemaphore);

impl<'a> SemaphorePermit<'a> {
    /// Block until a permit is acquired from `sem`.
    fn acquire(sem: &'a CountingSemaphore) -> Self {
        sem.acquire();
        Self(sem)
    }
}

impl Drop for SemaphorePermit<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// Read an optional `u32` argument at index `i`, falling back to `default`
/// when absent. Out-of-`u32`-range integers are `"out_of_range"`.
fn opt_u32(a: &[QueryValue], i: usize, default: u32) -> Result<u32, ScalarError> {
    if i >= a.len() {
        return Ok(default);
    }
    let n = arg_i64(a, i)?;
    u32::try_from(n).map_err(|_| ScalarError::new("out_of_range"))
}

/// `argon2id(password, salt, [memory_kb, time, parallelism, length]) -> Bin`.
fn argon2id_fn(a: &[QueryValue]) -> Result<QueryValue, ScalarError> {
    let password = arg_bytes(a, 0)?;
    let salt = arg_bytes(a, 1)?;
    let memory_kb = opt_u32(a, 2, A2_DEFAULT_MEMORY_KB)?;
    let time = opt_u32(a, 3, A2_DEFAULT_TIME)?;
    let parallelism = opt_u32(a, 4, A2_DEFAULT_PARALLELISM)?;
    let length = opt_u32(a, 5, A2_DEFAULT_LENGTH)? as usize;

    if memory_kb > A2_MAX_MEMORY_KB
        || time > A2_MAX_TIME
        || parallelism > A2_MAX_PARALLELISM
        || length > A2_MAX_LENGTH as usize
    {
        return Err(ScalarError::new("out_of_range"));
    }

    let cfg = Argon2Params::new(memory_kb, time, parallelism, Some(length))
        .map_err(|_| ScalarError::new("bad_params"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, cfg);
    let mut out = vec![0u8; length];
    // Gate the expensive KDF work behind the process-wide concurrency cap
    // (audit §2b). The permit is held ONLY across `hash_password_into` —
    // argument parsing/validation above runs unthrottled, so a bad-params
    // call does not consume a permit. Blocking-acquire is safe w.r.t. the
    // KDF's determinism: the semaphore affects *scheduling* only, never the
    // (password, salt, params) → digest mapping.
    let _permit = SemaphorePermit::acquire(&ARGON2ID_CONCURRENCY_GATE);
    // Observability: track in-flight and peak concurrency so the regression
    // test can assert the cap deterministically. Two relaxed atomics —
    // negligible vs. the KDF. Does NOT gate behaviour.
    let prev = A2_IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    A2_PEAK_IN_FLIGHT.fetch_max(prev + 1, Ordering::Relaxed);
    let res = argon
        .hash_password_into(password, salt, &mut out)
        .map_err(|_| ScalarError::new("compute"));
    A2_IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
    res?;
    Ok(v_bytes(out))
}

/// Register the `/crypto` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "sha256",
        FnEntry::pure(
            |a| {
                let bytes = arg_bytes(a, 0)?;
                Ok(v_bytes(Sha256::digest(bytes).to_vec()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "sha512",
        FnEntry::pure(
            |a| {
                let bytes = arg_bytes(a, 0)?;
                Ok(v_bytes(Sha512::digest(bytes).to_vec()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "sha3_256",
        FnEntry::pure(
            |a| {
                let bytes = arg_bytes(a, 0)?;
                Ok(v_bytes(Sha3_256::digest(bytes).to_vec()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "blake3",
        FnEntry::pure(
            |a| {
                let bytes = arg_bytes(a, 0)?;
                Ok(v_bytes(blake3::hash(bytes).as_bytes().to_vec()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "hmac_sha256",
        FnEntry::pure(
            |a| {
                let key = arg_bytes(a, 0)?;
                let msg = arg_bytes(a, 1)?;
                // SimpleHmac accepts keys of any length; new_from_slice is infallible here.
                let mut mac = SimpleHmac::<Sha256>::new_from_slice(key)
                    .map_err(|_| ScalarError::new("bad_key"))?;
                mac.update(msg);
                Ok(v_bytes(mac.finalize().into_bytes().to_vec()))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "ct_eq",
        FnEntry::pure(
            |a| {
                let lhs = arg_bytes(a, 0)?;
                let rhs = arg_bytes(a, 1)?;
                // subtle's ct_eq is constant-time only for equal-length inputs;
                // a length mismatch is a definite inequality.
                let eq = lhs.len() == rhs.len() && bool::from(lhs.ct_eq(rhs));
                Ok(v_bool(eq))
            },
            2,
            Some(2),
        ),
    );
    // argon2id(password, salt, [memory_kb, time, parallelism, length]).
    reg.register("argon2id", FnEntry::pure(argon2id_fn, 2, Some(6)));
}

#[cfg(test)]
mod tests;

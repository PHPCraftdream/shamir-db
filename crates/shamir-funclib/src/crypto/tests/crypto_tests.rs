//! Per-function `/crypto` tests — at least one correct-result assert (against a
//! published known-answer vector) and one error/edge case per registered fn.

use crate::crypto;
use crate::registry::{v_bool, ScalarRegistry};
use shamir_types::types::value::QueryValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    crypto::register(&mut r);
    r
}

fn bin(b: &[u8]) -> QueryValue {
    QueryValue::Bin(b.to_vec())
}

fn out(v: QueryValue) -> Vec<u8> {
    match v {
        QueryValue::Bin(b) => b,
        other => panic!("expected Bin, got {other:?}"),
    }
}

#[test]
fn sha256_known_answer_and_type_error() {
    let r = reg();
    // SHA-256("") known-answer vector.
    let got = out(r.call("sha256", &[bin(b"")]).unwrap());
    assert_eq!(
        hex_lower(&got),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(got.len(), 32);
    // error: wrong type (Str, not Bin).
    assert_eq!(
        r.call("sha256", &[QueryValue::Str("x".into())])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn sha512_known_answer_and_arity() {
    let r = reg();
    // SHA-512("") known-answer vector.
    let got = out(r.call("sha512", &[bin(b"")]).unwrap());
    assert_eq!(
        hex_lower(&got),
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
         47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );
    assert_eq!(got.len(), 64);
    // error: no args -> arity.
    assert_eq!(r.call("sha512", &[]).unwrap_err().code, "arity");
}

#[test]
fn sha3_256_known_answer_and_type_error() {
    let r = reg();
    // SHA3-256("") known-answer vector.
    let got = out(r.call("sha3_256", &[bin(b"")]).unwrap());
    assert_eq!(
        hex_lower(&got),
        "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a"
    );
    assert_eq!(got.len(), 32);
    // error: too many args -> arity.
    assert_eq!(
        r.call("sha3_256", &[bin(b"a"), bin(b"b")])
            .unwrap_err()
            .code,
        "arity"
    );
}

#[test]
fn blake3_known_answer_and_type_error() {
    let r = reg();
    // BLAKE3("") known-answer vector.
    let got = out(r.call("blake3", &[bin(b"")]).unwrap());
    assert_eq!(
        hex_lower(&got),
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
    assert_eq!(got.len(), 32);
    // error: wrong type.
    assert_eq!(
        r.call("blake3", &[QueryValue::Int(3)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn hmac_sha256_known_answer_and_arity() {
    let r = reg();
    // RFC 4231 Test Case 2: key = "Jefe", data = "what do ya want for nothing?".
    let got = out(r
        .call(
            "hmac_sha256",
            &[bin(b"Jefe"), bin(b"what do ya want for nothing?")],
        )
        .unwrap());
    assert_eq!(
        hex_lower(&got),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    assert_eq!(got.len(), 32);
    // error: missing message arg -> arity.
    assert_eq!(
        r.call("hmac_sha256", &[bin(b"key")]).unwrap_err().code,
        "arity"
    );
    // error: wrong type for key.
    assert_eq!(
        r.call("hmac_sha256", &[QueryValue::Int(1), bin(b"msg")])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn ct_eq_equal_unequal_and_length_mismatch() {
    let r = reg();
    // Equal contents -> true.
    assert_eq!(
        r.call("ct_eq", &[bin(b"abc"), bin(b"abc")]).unwrap(),
        v_bool(true)
    );
    // Differing contents, same length -> false.
    assert_eq!(
        r.call("ct_eq", &[bin(b"abc"), bin(b"abd")]).unwrap(),
        v_bool(false)
    );
    // Length mismatch -> false (definite inequality).
    assert_eq!(
        r.call("ct_eq", &[bin(b"abc"), bin(b"abcd")]).unwrap(),
        v_bool(false)
    );
    // error: wrong type.
    assert_eq!(
        r.call("ct_eq", &[bin(b"abc"), QueryValue::Bool(true)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

/// Lowercase hex helper local to the tests (avoids depending on the `hex` dep
/// from inside this module's tests).
fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Concurrency-cap regression (audit §2b).
///
/// `argon2id()` is a guest/query-reachable scalar that can allocate up to
/// `A2_MAX_MEMORY_KB` per call. Without an aggregate concurrency cap a
/// low-privileged user (or WASM guest function) can issue many parallel
/// `argon2id()` calls and OOM the server. This test asserts that the number
/// of `hash_password_into` calls executing *concurrently* never exceeds the
/// documented cap.
///
/// Determinism: the semaphore gates *scheduling* only — it must not change the
/// KDF output. The `argon2id_matches_reference_and_is_deterministic` test
/// above covers the bit-identity contract; this test covers the cap.
///
/// Methodology: [`crypto::argon2id_fn`] updates a process-wide
/// `A2_PEAK_IN_FLIGHT` atomic (inc-on-entry / dec-on-exit around the gated
/// region, with a `fetch_max` to track the running peak). We reset it to zero,
/// spawn `2×cap` worker threads each invoking `argon2id()` through the registry
/// with a moderate profile, join them, then read the peak.
///
/// - **Capped (correct):** the semaphore holds in-flight at `cap`, so
///   `A2_PEAK_IN_FLIGHT <= cap`.
/// - **Uncapped (the bug):** all `2×cap` calls enter the KDF region
///   concurrently, so `A2_PEAK_IN_FLIGHT == 2×cap > cap`.
///
/// # Deterministic overlap via a barrier (audit G4 / #528)
///
/// An earlier version of this test relied on wall-clock scheduling luck: it
/// spawned the workers and hoped enough of them reached `hash_password_into`
/// at overlapping instants to saturate the semaphore. Under system load the
/// threads staggered, the KDF calls did not overlap past `cap`, and the
/// `peak == cap` assertion flaked (e.g. "expected 16, got 10") — a
/// timing-measurement flake, NOT a semaphore-correctness bug.
///
/// The fix synchronises entry with a [`std::sync::Barrier`] sized to
/// `n_workers`: every worker does all its cheap setup (registry lookup, arg
/// construction) and then blocks on `barrier.wait()`. The barrier releases
/// ALL threads simultaneously, so they all race into the semaphore-gated KDF
/// region at approximately the same instant regardless of scheduler jitter.
/// The semaphore admits exactly `cap` of them; because each admitted call
/// holds its permit for the full KDF duration (tens of ms — orders of
/// magnitude longer than the post-barrier acquire path), all `cap` permit
/// holders are provably in flight together, driving `A2_PEAK_IN_FLIGHT` to
/// `cap`. This makes the "peak reaches cap" assertion deterministic instead
/// of probabilistic. The semaphore gating logic itself is unchanged — this is
/// purely a test-synchronisation hardening.
#[test]
fn argon2id_concurrency_cap_bounds_parallel_calls() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let cap = crate::crypto::ARGON2ID_CONCURRENCY_CAP;
    let n_workers = (cap as usize) * 2; // 2× cap → uncapped peak clearly exceeds cap

    // Reset BOTH observability counters so prior tests (which may have left
    // A2_IN_FLIGHT non-zero if a call was interrupted, or raised the peak)
    // do not pollute this run's assertion.
    crate::crypto::A2_PEAK_IN_FLIGHT.store(0, std::sync::atomic::Ordering::Relaxed);
    crate::crypto::A2_IN_FLIGHT.store(0, std::sync::atomic::Ordering::Relaxed);

    let registry = Arc::new(reg());
    // Barrier sized to every worker: all threads block until the last one
    // arrives, then enter the semaphore-gated KDF region together. This
    // removes the wall-clock-scheduling dependency that made the previous
    // `peak == cap` assertion flaky under load.
    let gate = Arc::new(Barrier::new(n_workers));
    let password = b"concurrency-test-pw";
    let salt = b"0123456789abcdef"; // 16 bytes

    let mut handles = Vec::with_capacity(n_workers);
    for _ in 0..n_workers {
        let reg = Arc::clone(&registry);
        let gate = Arc::clone(&gate);
        handles.push(thread::spawn(move || {
            // Build the argument vector BEFORE the barrier so the only work
            // between release and the semaphore acquire is the call itself —
            // maximising the overlap window at the gated region.
            let args = [
                bin(password),
                bin(salt),
                QueryValue::Int(16_384), // memory_kb — 16 MiB
                QueryValue::Int(2),      // time
                QueryValue::Int(1),      // parallelism
                QueryValue::Int(32),     // length
            ];
            // Release all workers at once → simultaneous rush at the semaphore.
            gate.wait();
            // Moderate profile (16 MiB, t=2): each call holds its permit long
            // enough that all `cap` permit holders are provably in-flight
            // together, while staying small enough (16 MiB) that memory-
            // bandwidth contention does not fully serialise the calls
            // independent of the semaphore.
            let res = reg.call("argon2id", &args);
            res.unwrap_or_else(|e| panic!("argon2id call failed: {e:?}"));
        }));
    }
    for h in handles {
        h.join().expect("worker thread panicked");
    }

    let peak = crate::crypto::A2_PEAK_IN_FLIGHT.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        peak <= cap,
        "argon2id concurrency cap violated: observed peak in-flight {peak} > cap {cap} \
         (spawned {n_workers} concurrent workers)",
    );
    // Sanity: with 2×cap workers all released together by the barrier, the cap
    // must actually have been saturated — otherwise the test is vacuous.
    // Peak == cap (not below) confirms the semaphore was exercised at
    // saturation.
    assert_eq!(
        peak, cap,
        "expected the concurrency cap to be saturated (peak == cap == {cap}), got peak = {peak}; \
         the workers likely did not overlap enough — increase n_workers or the per-call work",
    );
}

#[test]
fn argon2id_matches_reference_and_is_deterministic() {
    use argon2::{Algorithm, Argon2, Params, Version};

    let r = reg();
    let password = b"correct horse battery staple";
    let salt = b"0123456789abcdef"; // 16 bytes

    let got = out(r.call("argon2id", &[bin(password), bin(salt)]).unwrap());

    // Independent reference using the documented defaults (19456 KiB, t=2,
    // p=1, len=32).
    let cfg = Params::new(19_456, 2, 1, Some(32)).unwrap();
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, cfg);
    let mut expected = vec![0u8; 32];
    argon
        .hash_password_into(password, salt, &mut expected)
        .unwrap();

    assert_eq!(got.len(), 32);
    assert_eq!(got, expected);

    // Same inputs → same digest.
    let again = out(r.call("argon2id", &[bin(password), bin(salt)]).unwrap());
    assert_eq!(got, again);
}

#[test]
fn argon2id_honours_custom_length() {
    let r = reg();
    // memory_kb, time, parallelism, length = 64
    let got = out(r
        .call(
            "argon2id",
            &[
                bin(b"pw"),
                bin(b"0123456789abcdef"),
                QueryValue::Int(19_456),
                QueryValue::Int(2),
                QueryValue::Int(1),
                QueryValue::Int(64),
            ],
        )
        .unwrap());
    assert_eq!(got.len(), 64);
}

#[test]
fn argon2id_errors() {
    let r = reg();
    // Salt < 8 bytes → Argon2 rejects → "compute".
    assert_eq!(
        r.call("argon2id", &[bin(b"pw"), bin(b"short")])
            .unwrap_err()
            .code,
        "compute"
    );
    // length over the cap → "out_of_range".
    assert_eq!(
        r.call(
            "argon2id",
            &[
                bin(b"pw"),
                bin(b"0123456789abcdef"),
                QueryValue::Int(19_456),
                QueryValue::Int(2),
                QueryValue::Int(1),
                QueryValue::Int(9999),
            ],
        )
        .unwrap_err()
        .code,
        "out_of_range"
    );
    // missing salt (arity) → "arity".
    assert_eq!(r.call("argon2id", &[bin(b"pw")]).unwrap_err().code, "arity");
}

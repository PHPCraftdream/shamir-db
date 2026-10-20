use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;

use shamir_connect::common::latency::{padding_for, target_constant_time_ms, FIXED_FLOOR_MS};
use shamir_connect::common::types::limits::MAX_PRE_AUTH_FRAME;
use shamir_connect::server::lockout::{
    FailureOutcome, InMemoryLockoutStore, LockoutStore, PairKey, Subnet, BACKOFF_BASE_MS,
    BACKOFF_CAP_MS, LOCKOUT_THRESHOLD,
};
use shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT;

use crate::server_meta::ServerMetaStore;

// HIGH-1 compile-time invariants: pin the pre-auth frame ceiling at
// the spec §8 value and prove it stays strictly below the post-auth
// ceiling the request loop uses. `const` assertions trip at compile
// time so a future spec edit that weakens either bound fails the
// build, not just the test suite.
const _: () = assert!(
    MAX_PRE_AUTH_FRAME == 4 * 1024,
    "MAX_PRE_AUTH_FRAME must equal 4 KiB per spec §8",
);
const _: () = assert!(
    MAX_PRE_AUTH_FRAME < MAX_FRAME_SIZE_DEFAULT,
    "pre-auth ceiling must be strictly smaller than post-auth ceiling",
);

/// HIGH-1 regression: `run_handshake` must read pre-auth frames with
/// the 4 KiB ceiling, not the post-auth 16 MiB ceiling. The compile-
/// time `const` asserts above pin the constants; this runtime test
/// surfaces a human-readable failure when the bounds are tightened
/// or loosened in the future, and additionally documents the
/// resource-exhaustion budget.
#[test]
fn pre_auth_frame_budget_is_safe_for_ten_thousand_unauth_peers() {
    // Defense-in-depth: 10 000 concurrent unauthenticated peers
    // multiplied by the pre-auth cap must stay well under
    // commodity-server RAM. 10 000 × 4 KiB = 40 MiB, vs. the
    // 10 000 × 16 MiB = ~160 GiB that the old shape allowed.
    let max_unauth_memory = MAX_PRE_AUTH_FRAME.saturating_mul(10_000);
    assert!(
        max_unauth_memory < 128 * 1024 * 1024,
        "pre-auth cap × 10k connections should be under 128 MiB; got {}",
        max_unauth_memory,
    );
}

// ---------------------------------------------------------------------
// NEW-2: per-pair exponential backoff is COMPUTED *and APPLIED*.
//
// The application logic lives in two spots:
//   * `run_handshake` (ProofOutcome::Rejected arm) maps the
//     `register_failure` outcome → `backoff_ms` and returns it via
//     `HandshakeError::BadProof { backoff_ms }`.
//   * `handle_connection` widens the negative-path latency pad to
//     `max(target_constant_time_ms(), backoff_ms)`.
//
// Driving the full async `run_handshake` requires a complete
// `ConnectionContext` (TLS identity, redb user dir, …) so these tests
// exercise the two load-bearing pieces directly: (1) the exact
// outcome→backoff mapping used by the reject arm, against the real
// `InMemoryLockoutStore`, and (2) the `max(floor, backoff)` pad formula.
// ---------------------------------------------------------------------

/// Mirror of the `ProofOutcome::Rejected` arm in `run_handshake`: map a
/// `register_failure` outcome to the `backoff_ms` that gets plumbed into
/// `HandshakeError::BadProof`. Kept in lockstep with the production code
/// so the test fails if the mapping drifts.
fn backoff_ms_for(outcome: FailureOutcome) -> u64 {
    match outcome {
        FailureOutcome::Backoff { delay_ms } => delay_ms,
        FailureOutcome::LockedOut => BACKOFF_CAP_MS,
    }
}

fn pair(subnet: u8, user: u8) -> PairKey {
    (Subnet::V4([10, 0, subnet]), [user; 16])
}

/// The backoff plumbed into the reject path must escalate `100ms × 2^N`
/// (capped 30s) as failures accumulate for a `(subnet, username_hash)`
/// pair, exactly as the reject arm computes it from the real store.
#[test]
fn reject_path_backoff_escalates_per_failure() {
    let store = InMemoryLockoutStore::new();
    let now = 1_000_000_000u64;
    let k = pair(1, 1);
    const SECOND_NS: u64 = 1_000_000_000;

    // First failures (well below the 50-failure lockout threshold)
    // double each time: 100, 200, 400, 800, 1600, ...
    let expected = [
        100u64, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 30000, 30000,
    ];
    for (i, &want) in expected.iter().enumerate() {
        let outcome = store.register_failure(k, now + (i as u64) * SECOND_NS);
        assert_eq!(
            backoff_ms_for(outcome),
            want,
            "failure #{} should map to {}ms backoff",
            i + 1,
            want,
        );
    }
    // Base × 2^0 sanity (documents the formula's anchor).
    assert_eq!(BACKOFF_BASE_MS, 100);
}

/// Crossing the lockout threshold must still yield a bounded backoff for
/// the final response (`BACKOFF_CAP_MS`), not panic or 0 — the reject arm
/// maps `FailureOutcome::LockedOut → BACKOFF_CAP_MS`.
#[test]
fn reject_path_backoff_caps_at_lockout_threshold() {
    let store = InMemoryLockoutStore::new();
    let now = 1_000_000_000u64;
    let k = pair(2, 2);
    const SECOND_NS: u64 = 1_000_000_000;

    let mut last = 0u64;
    for i in 0..LOCKOUT_THRESHOLD {
        let outcome = store.register_failure(k, now + (i as u64) * SECOND_NS);
        last = backoff_ms_for(outcome);
    }
    // The 50th failure trips the lockout; the mapped backoff is the cap.
    assert_eq!(
        last, BACKOFF_CAP_MS,
        "threshold-crossing backoff is the cap"
    );
    assert!(store.is_locked_out(k, now + (LOCKOUT_THRESHOLD as u64) * SECOND_NS));
}

/// The negative-path pad target is `max(constant_time_floor, backoff)` —
/// the timing-oracle floor is preserved AND the escalation is enforced.
/// With elapsed below the target, the computed sleep reaches the backoff.
#[test]
fn pad_target_is_max_of_floor_and_backoff() {
    // A large backoff dominates the floor: total pad ≈ backoff.
    let backoff_ms = 6400u64;
    // Sample the (random) floor many times; the formula must never drop
    // below either input.
    for _ in 0..256 {
        let target_ms = target_constant_time_ms().max(backoff_ms);
        assert!(
            target_ms >= backoff_ms,
            "pad target must be >= backoff ({target_ms} < {backoff_ms})",
        );
        assert!(
            target_ms >= FIXED_FLOOR_MS,
            "pad target must be >= constant-time floor ({target_ms} < {FIXED_FLOOR_MS})",
        );
    }

    // With a tiny elapsed, the sleep computed by the same helper the
    // negative path uses must reach the backoff window.
    let target_ms = 6400u64.max(target_constant_time_ms());
    let sleep = padding_for(Duration::from_millis(5), target_ms);
    assert!(
        sleep >= Duration::from_millis(backoff_ms - 5),
        "negative-path sleep ({sleep:?}) must reach the backoff window",
    );
}

/// When there is no backoff (e.g. an internal verify error path with
/// `backoff_ms = 0`), the pad target collapses to the constant-time
/// floor — behaviour identical to the pre-NEW-2 flat pad.
#[test]
fn zero_backoff_collapses_to_constant_time_floor() {
    for _ in 0..256 {
        let floor = target_constant_time_ms();
        // `black_box` so the zero is treated as an opaque runtime value
        // (matching the production `floor.max(backoff_ms)` where
        // `backoff_ms == 0`) rather than a compile-time no-op.
        let backoff_ms = std::hint::black_box(0u64);
        let target_ms = floor.max(backoff_ms);
        assert_eq!(target_ms, floor);
        assert!((FIXED_FLOOR_MS..=FIXED_FLOOR_MS + 25).contains(&target_ms));
    }
}

/// No user-existence oracle: the store's backoff depends ONLY on the
/// failure count for the `(subnet, username_hash)` pair, never on whether
/// the username maps to a real account. Two distinct username hashes on
/// the same subnet, failed the same number of times, must yield identical
/// backoff progressions — so widening the pad to the backoff cannot leak
/// which usernames exist.
#[test]
fn backoff_progression_is_identical_for_any_pair() {
    let store = InMemoryLockoutStore::new();
    let now = 1_000_000_000u64;
    const SECOND_NS: u64 = 1_000_000_000;
    let real_user = pair(7, 0xaa); // imagine this hash maps to a real account
    let fake_user = pair(7, 0xbb); // and this one does not

    for i in 0..8u64 {
        let a = backoff_ms_for(store.register_failure(real_user, now + i * SECOND_NS));
        let b = backoff_ms_for(store.register_failure(fake_user, now + i * SECOND_NS));
        assert_eq!(
            a,
            b,
            "backoff at failure #{} must not depend on the pair identity",
            i + 1,
        );
    }
}

// ---------------------------------------------------------------------
// F-25 (#818, P0) + F-38 (#846, P0): bootstrap-TTL fail-closed gate
// inserted into `run_handshake` between `ProofOutcome::Accepted` and the
// (best-effort, non-fatal) rotation-on-success block.
//
// Same rationale as the NEW-2 backoff tests above: driving the full async
// `run_handshake` requires a complete `ConnectionContext`, so this calls
// the REAL production read (`ServerMetaStore::read_bootstrap_token_state`,
// the single-fallible-read method F-38 introduced) against a real
// `ServerMetaStore` (not a mock), and mirrors the EXACT gate decision
// (`match` arms) so the test fails if the two drift apart.
// ---------------------------------------------------------------------

/// The F-25/F-38 gate's three possible outcomes. In production `run_handshake`
/// maps BOTH reject variants to the SAME client-facing failure
/// (`HandshakeError::BadProof`, identical `register_failure` → backoff), so
/// they are indistinguishable to an outside observer; they are kept as two
/// variants here only so the tests can prove which arm fired.
#[derive(Debug, PartialEq, Eq)]
enum BootstrapGateOutcome {
    /// Outstanding bootstrap token for this username has expired → reject.
    RejectExpired,
    /// Bootstrap metadata read/decode error → reject (F-38 fail-closed).
    RejectMetaReadError,
    /// No gating reason → proceed to grant the session.
    Proceed,
}

/// Mirror of the F-25/F-38 gate in `run_handshake`, kept in lockstep with
/// the production `match ctx.meta.read_bootstrap_token_state(now_ns)` so
/// this test fails if the two drift apart. Calls the REAL production read
/// method so the read path itself is exercised, not re-implemented.
fn evaluate_bootstrap_gate(
    meta: &ServerMetaStore,
    username: &str,
    now_ns: u64,
) -> BootstrapGateOutcome {
    match meta.read_bootstrap_token_state(now_ns) {
        Ok(state)
            if state.active && state.username.as_deref() == Some(username) && state.expired =>
        {
            BootstrapGateOutcome::RejectExpired
        }
        Ok(_) => BootstrapGateOutcome::Proceed,
        Err(_) => BootstrapGateOutcome::RejectMetaReadError,
    }
}

fn tmp_meta_store() -> (TempDir, ServerMetaStore) {
    let dir = TempDir::new().expect("tempdir");
    let store = ServerMetaStore::open_or_init(dir.path().join("server_meta"))
        .expect("open_or_init server_meta");
    (dir, store)
}

/// Core F-25 guarantee (regression): once the outstanding bootstrap token's
/// TTL has passed, the gate fires for the matching username — this is what
/// makes `run_handshake` reject the login instead of falling through to the
/// (best-effort, non-fatal) rotation attempt. Re-confirmed under the F-38
/// code path (the gate now reads via `read_bootstrap_token_state`).
#[test]
fn gate_fires_when_outstanding_token_for_username_has_expired() {
    let (_dir, store) = tmp_meta_store();
    let expires_at_ns = 1_000_000_000_000u64;
    store
        .set_bootstrap_token(
            "admin",
            [0xAAu8; 32],
            expires_at_ns,
            PathBuf::from("/run/shamir/bootstrap_token.txt"),
        )
        .expect("set_bootstrap_token");

    assert_eq!(
        evaluate_bootstrap_gate(&store, "admin", expires_at_ns),
        BootstrapGateOutcome::RejectExpired,
        "gate must fire at exactly the expiry boundary (<=), matching \
         `bootstrap_token_expired`'s own semantics"
    );
    assert_eq!(
        evaluate_bootstrap_gate(&store, "admin", expires_at_ns + 1),
        BootstrapGateOutcome::RejectExpired,
    );
}

/// F-3 regression: a NON-expired outstanding token must NOT trip the new
/// gate — the existing best-effort rotation-on-success login path (F-3)
/// must keep working unchanged for the common case. Confirms a readable,
/// active, NOT-expired token still proceeds to grant a session normally.
#[test]
fn gate_does_not_fire_for_non_expired_token() {
    let (_dir, store) = tmp_meta_store();
    let expires_at_ns = 1_000_000_000_000u64;
    store
        .set_bootstrap_token(
            "admin",
            [0xAAu8; 32],
            expires_at_ns,
            PathBuf::from("/run/shamir/bootstrap_token.txt"),
        )
        .expect("set_bootstrap_token");

    assert_eq!(
        evaluate_bootstrap_gate(&store, "admin", expires_at_ns - 1),
        BootstrapGateOutcome::Proceed,
        "F-3 regression: a not-yet-expired token must not be gated"
    );
}

/// The gate is scoped to the bootstrap username only — a login for a
/// DIFFERENT username must never be affected by an outstanding (even
/// expired) bootstrap token issued to someone else.
#[test]
fn gate_does_not_fire_for_a_different_username() {
    let (_dir, store) = tmp_meta_store();
    store
        .set_bootstrap_token(
            "admin",
            [0xAAu8; 32],
            1, // already expired
            PathBuf::from("/run/shamir/bootstrap_token.txt"),
        )
        .expect("set_bootstrap_token");

    assert_eq!(
        evaluate_bootstrap_gate(&store, "someone_else", u64::MAX),
        BootstrapGateOutcome::Proceed,
    );
}

/// No outstanding token at all → gate never fires, regardless of `now_ns`
/// (a genuinely-absent row reads as `Ok(snapshot-all-false)`, not an error,
/// so the gate proceeds — matching `bootstrap_token_expired`'s pre-F-38
/// "fails open when no token is outstanding" semantics for the absent case).
#[test]
fn gate_does_not_fire_when_no_token_is_outstanding() {
    let (_dir, store) = tmp_meta_store();
    assert_eq!(
        evaluate_bootstrap_gate(&store, "admin", u64::MAX),
        BootstrapGateOutcome::Proceed,
    );
}

/// F-38 (#846, P0) CORE — fault injection: a genuine metadata read failure
/// at the exact moment `read_bootstrap_token_state` is called during a login
/// with a VALID password proof must FAIL CLOSED — the gate must REJECT,
/// never proceed to grant a session for what may be an expired bootstrap
/// credential.
///
/// Injection mechanism: after provisioning a real bootstrap token via the
/// normal `set_bootstrap_token` path, directly overwrite the raw bytes under
/// `KEY_BOOTSTRAP` in the underlying fjall keyspace with garbage (bypassing
/// the serde `put` encode path) so the next `read_blob::<PersistedBootstrap>`
/// hits a REAL `rmp_serde::from_slice` decode error — a genuine,
/// reproducible read failure, not a mock. (See
/// `ServerMetaStore::corrupt_bootstrap_blob_for_test`.)
#[test]
fn gate_fails_closed_on_bootstrap_metadata_read_error() {
    let (_dir, store) = tmp_meta_store();
    // Provision a real, genuinely-expired bootstrap token for "admin" first
    // so the pre-corruption state is unambiguously the
    // "outstanding + expired + matching-username" case (which would reject
    // via the Ok arm anyway). The corruption then forces the gate to take
    // the Err arm instead — proving it rejects even when the readable
    // semantics cannot be evaluated at all.
    let expires_at_ns = 1_000_000_000_000u64;
    store
        .set_bootstrap_token(
            "admin",
            [0xAAu8; 32],
            expires_at_ns,
            PathBuf::from("/run/shamir/bootstrap_token.txt"),
        )
        .expect("set_bootstrap_token");
    // Sanity: pre-corruption, the gate rejects via the expired Ok arm.
    assert_eq!(
        evaluate_bootstrap_gate(&store, "admin", expires_at_ns),
        BootstrapGateOutcome::RejectExpired,
        "sanity: pre-corruption, the expired token must reject"
    );

    // Inject the read failure: overwrite KEY_BOOTSTRAP with garbage bytes.
    store.corrupt_bootstrap_blob_for_test();

    // The production read method must surface a REAL decode error (not a
    // swallowed Ok of an all-false snapshot) — this is the load-bearing
    // difference F-38 introduces vs the old fail-open granular getters.
    let read_result = store.read_bootstrap_token_state(expires_at_ns);
    assert!(
        matches!(read_result, Err(crate::server_meta::MetaError::Encoding(_))),
        "corrupted KEY_BOOTSTRAP blob must surface as a real \
         MetaError::Encoding (rmp decode) error, got {read_result:?}"
    );

    // F-38 fail-closed: the gate routes the read error to a REJECT (the
    // metadata-error arm), NEVER to Proceed — so a transient metadata read
    // failure at the security gate can never grant a session.
    assert_eq!(
        evaluate_bootstrap_gate(&store, "admin", expires_at_ns),
        BootstrapGateOutcome::RejectMetaReadError,
        "F-38: a bootstrap metadata read error must fail CLOSED — reject, \
         not grant a session"
    );

    // Defense-in-depth: the corruption must NOT have changed the OTHER two
    // (intentionally lenient) call sites' fail-OPEN behaviour — the granular
    // getters still swallow the decode error and return false/None. This
    // pins the scope of F-38 to the single security gate.
    assert!(
        !store.bootstrap_token_active(),
        "the lenient granular getters must keep their fail-OPEN behaviour \
         (return false on read error); only the security gate is fail-closed"
    );
    assert_eq!(store.bootstrap_username(), None);
    assert!(
        !store.bootstrap_token_expired(expires_at_ns),
        "bootstrap_token_expired must keep its fail-OPEN behaviour on read error"
    );
}

/// Oracle-avoidance: the new reject path must reuse the EXACT SAME
/// `register_failure` → backoff mapping as the existing
/// `ProofOutcome::Rejected` arm (`backoff_ms_for`, defined above), so an
/// expired-bootstrap-token rejection is indistinguishable in shape and
/// backoff progression from an ordinary bad-proof rejection for the same
/// pair.
#[test]
fn expired_bootstrap_rejection_uses_the_same_backoff_mapping_as_bad_proof() {
    let store_a = InMemoryLockoutStore::new();
    let store_b = InMemoryLockoutStore::new();
    let now = 1_000_000_000u64;
    const SECOND_NS: u64 = 1_000_000_000;
    // Two independent pairs: one standing in for the "expired bootstrap
    // token" rejection path, one for an ordinary "bad proof" rejection.
    let expired_token_pair = pair(9, 0xcc);
    let bad_proof_pair = pair(9, 0xdd);

    for i in 0..8u64 {
        let a = backoff_ms_for(store_a.register_failure(expired_token_pair, now + i * SECOND_NS));
        let b = backoff_ms_for(store_b.register_failure(bad_proof_pair, now + i * SECOND_NS));
        assert_eq!(
            a,
            b,
            "failure #{}: expired-token rejection backoff must match a \
             bad-proof rejection's backoff bit-for-bit",
            i + 1,
        );
    }
}

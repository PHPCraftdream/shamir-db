//! Integration tests for [`shamir_server::server_meta::ServerMetaStore`].
//!
//! Spec source: `IMPLEMENTATION_GUIDE.md` §1.2 (NORMATIVE schema) + §1.3
//! (durability requirement). We use `tempfile::TempDir` so each test gets
//! its own fresh redb file, and we exercise crash-restart by `drop`-ing the
//! store between operations and re-opening from the same path.

use shamir_server::server_meta::{MetaError, ServerMetaStore};
use std::path::PathBuf;
use tempfile::TempDir;

fn tmp_path() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("server_meta.redb");
    (dir, path)
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

#[test]
fn init_creates_random_secrets() {
    let (_dir, path) = tmp_path();
    let store = ServerMetaStore::open_or_init(&path).expect("init");

    let secrets = store.server_secrets().expect("secrets");

    // Must not be all zeros.
    assert_ne!(
        secrets.server_secret, [0u8; 32],
        "server_secret should be random, not zero"
    );
    assert_ne!(
        secrets.lockout_secret, [0u8; 32],
        "lockout_secret should be random, not zero"
    );

    // server_secret and lockout_secret are independent (overwhelming
    // probability of inequality with 32 random bytes each).
    assert_ne!(
        secrets.server_secret, secrets.lockout_secret,
        "server_secret and lockout_secret must be derived independently"
    );

    let audit_chain_key = store.audit_chain_key().expect("audit_chain_key");
    assert_ne!(audit_chain_key, [0u8; 32], "audit_chain_key must be random");
    assert_ne!(
        audit_chain_key, secrets.server_secret,
        "audit_chain_key must be independent of server_secret"
    );

    let (ticket_current, ticket_previous) = store.ticket_keys().expect("ticket_keys");
    assert_ne!(ticket_current, [0u8; 32], "ticket_key must be random");
    assert!(
        ticket_previous.is_none(),
        "no previous ticket on fresh init"
    );

    // Identity rehydrates without panic; current_version starts at 0.
    let identity = store.identity_state().expect("identity_state");
    assert_eq!(identity.current_version(), 0);
    assert!(identity.previous_pub().is_none());

    // Bootstrap state on fresh init: empty + superuser_ever_existed = false.
    let boot = store.bootstrap_state();
    assert!(!boot.superuser_ever_existed());

    // No audit checkpoint yet.
    assert!(store.audit_checkpoint().is_none());

    // Created_at_ns is set.
    assert!(store.created_at_ns() > 0);
}

#[test]
fn init_persists_across_reopen() {
    let (_dir, path) = tmp_path();

    let (server_secret, lockout_secret, audit_key, ticket_current, created_at_ns, identity_pub) = {
        let store = ServerMetaStore::open_or_init(&path).expect("init");
        let secrets = store.server_secrets().expect("secrets");
        let identity = store.identity_state().expect("identity_state");
        (
            secrets.server_secret,
            secrets.lockout_secret,
            store.audit_chain_key().expect("audit_chain_key"),
            store.ticket_keys().expect("ticket_keys").0,
            store.created_at_ns(),
            identity.current_pub(),
        )
    };

    // Reopen the same path.
    let store = ServerMetaStore::open_or_init(&path).expect("reopen");
    let secrets2 = store.server_secrets().expect("secrets");
    assert_eq!(secrets2.server_secret, server_secret);
    assert_eq!(secrets2.lockout_secret, lockout_secret);
    assert_eq!(store.audit_chain_key().expect("audit_chain_key"), audit_key);
    assert_eq!(store.ticket_keys().expect("ticket_keys").0, ticket_current);
    assert_eq!(
        store.created_at_ns(),
        created_at_ns,
        "created_at_ns must NOT change on reopen"
    );
    assert_eq!(
        store
            .identity_state()
            .expect("identity_state")
            .current_pub(),
        identity_pub,
        "Ed25519 seed must rehydrate into the same pub key"
    );
}

// ---------------------------------------------------------------------------
// Ticket key rotation
// ---------------------------------------------------------------------------

#[test]
fn rotate_ticket_key_atomically() {
    let (_dir, path) = tmp_path();
    let store = ServerMetaStore::open_or_init(&path).expect("init");

    let (initial, prev) = store.ticket_keys().expect("ticket_keys");
    assert!(prev.is_none());

    let new_key = [0xa5u8; 32];
    let now = 1_700_000_000_000_000_000u64;
    store.rotate_ticket_key(new_key, now).expect("rotate");

    let (current_after, previous_after) = store.ticket_keys().expect("ticket_keys");
    assert_eq!(current_after, new_key);
    assert_eq!(
        previous_after,
        Some(initial),
        "previous slot must hold the pre-rotation key"
    );

    // Reopen → state preserved (durability test).
    drop(store);
    let store = ServerMetaStore::open_or_init(&path).expect("reopen");
    let (current_after_reopen, previous_after_reopen) = store.ticket_keys().expect("ticket_keys");
    assert_eq!(current_after_reopen, new_key);
    assert_eq!(previous_after_reopen, Some(initial));
}

// ---------------------------------------------------------------------------
// Identity rotation
// ---------------------------------------------------------------------------

#[test]
fn finalize_identity_rotation_clears_previous() {
    let (_dir, path) = tmp_path();
    let store = ServerMetaStore::open_or_init(&path).expect("init");

    let current_seed = [0x11u8; 32];
    let previous_seed = [0x22u8; 32];
    let until_ns = 1_800_000_000_000_000_000u64;
    let new_version = 3u64;

    store
        .store_identity_after_rotate(current_seed, previous_seed, until_ns, new_version)
        .expect("store_identity_after_rotate");

    // Reopen → previous_seed = Some, rotation_until_ns = Some.
    drop(store);
    let store = ServerMetaStore::open_or_init(&path).expect("reopen");
    let identity = store.identity_state().expect("identity_state");
    assert_eq!(identity.current_version(), new_version);
    assert!(
        identity.previous_pub().is_some(),
        "previous pub must be present during overlap"
    );
    assert_eq!(identity.rotation_until_ns(), Some(until_ns));

    // Finalize.
    store
        .finalize_identity_rotation()
        .expect("finalize_identity_rotation");

    let identity_after = store.identity_state().expect("identity_state");
    assert!(
        identity_after.previous_pub().is_none(),
        "previous pub must be cleared after finalize"
    );
    assert!(identity_after.rotation_until_ns().is_none());
    assert_eq!(
        identity_after.current_version(),
        new_version,
        "version must NOT regress on finalize"
    );

    // Reopen → still cleared.
    drop(store);
    let store = ServerMetaStore::open_or_init(&path).expect("reopen-2");
    let final_identity = store.identity_state().expect("identity_state");
    assert!(final_identity.previous_pub().is_none());
    assert!(final_identity.rotation_until_ns().is_none());
}

// ---------------------------------------------------------------------------
// Bootstrap consume
// ---------------------------------------------------------------------------

#[test]
fn consume_bootstrap_token_is_idempotent_atomic() {
    let (_dir, path) = tmp_path();
    let store = ServerMetaStore::open_or_init(&path).expect("init");

    // Initial state — no token, never seen a superuser.
    assert!(!store.superuser_ever_existed());
    assert!(!store.bootstrap_token_active());

    let hash = [0x77u8; 32];
    let expires = 2_000_000_000_000_000_000u64;
    let token_path = PathBuf::from("/tmp/bootstrap_token.txt");
    store
        .set_bootstrap_token("admin", hash, expires, token_path.clone())
        .expect("set_bootstrap_token");
    assert!(store.bootstrap_token_active());
    assert!(!store.superuser_ever_existed());
    assert_eq!(store.bootstrap_username(), Some("admin".to_string()));
    assert_eq!(store.bootstrap_token_path(), Some(token_path));

    // Consume.
    store.consume_bootstrap_token().expect("consume");

    // Reopen → token gone, superuser_ever_existed sticky.
    drop(store);
    let store = ServerMetaStore::open_or_init(&path).expect("reopen");
    assert!(!store.bootstrap_token_active());
    assert!(store.superuser_ever_existed());
    assert_eq!(store.bootstrap_username(), None);
    assert_eq!(store.bootstrap_token_path(), None);

    // Idempotent — second consume is a no-op (no error, state unchanged).
    store
        .consume_bootstrap_token()
        .expect("second consume must not error");
    assert!(!store.bootstrap_token_active());
    assert!(store.superuser_ever_existed());
}

// ---------------------------------------------------------------------------
// Audit checkpoint
// ---------------------------------------------------------------------------

#[test]
fn audit_checkpoint_round_trips() {
    let (_dir, path) = tmp_path();
    let store = ServerMetaStore::open_or_init(&path).expect("init");

    assert!(store.audit_checkpoint().is_none());

    let seq = 42u64;
    let hmac = [0xabu8; 32];
    store
        .store_audit_checkpoint(seq, hmac)
        .expect("store_audit_checkpoint");
    assert_eq!(store.audit_checkpoint(), Some((seq, hmac)));

    // Reopen → state preserved.
    drop(store);
    let store = ServerMetaStore::open_or_init(&path).expect("reopen");
    assert_eq!(store.audit_checkpoint(), Some((seq, hmac)));

    // Overwrite with a higher seq.
    let seq2 = 100u64;
    let hmac2 = [0xcdu8; 32];
    store
        .store_audit_checkpoint(seq2, hmac2)
        .expect("store_audit_checkpoint #2");
    assert_eq!(store.audit_checkpoint(), Some((seq2, hmac2)));
}

// ---------------------------------------------------------------------------
// Crash simulation: 5 distinct rotations all survive a restart cycle.
// ---------------------------------------------------------------------------

#[test]
fn crash_simulation_preserves_state() {
    let (_dir, path) = tmp_path();

    // Step 1: init.
    let initial_ticket;
    let initial_audit;
    let initial_secret;
    {
        let store = ServerMetaStore::open_or_init(&path).expect("init");
        initial_ticket = store.ticket_keys().expect("ticket_keys").0;
        initial_audit = store.audit_chain_key().expect("audit_chain_key");
        initial_secret = store.server_secrets().expect("secrets").server_secret;
    }

    // Step 2: rotate ticket_key + crash.
    let new_ticket = [0x01u8; 32];
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-1");
        store
            .rotate_ticket_key(new_ticket, 1_000)
            .expect("rotate_ticket_key");
        // Drop = simulated crash.
    }

    // Step 3: rotate audit_chain_key + crash.
    let new_audit = [0x02u8; 32];
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-2");
        // Verify prior rotation persisted.
        assert_eq!(store.ticket_keys().expect("ticket_keys").0, new_ticket);
        assert_eq!(
            store.ticket_keys().expect("ticket_keys").1,
            Some(initial_ticket)
        );

        store
            .rotate_audit_chain_key(new_audit, 2_000)
            .expect("rotate_audit_chain_key");
    }

    // Step 4: rotate server_secret + crash.
    let new_secret = [0x03u8; 32];
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-3");
        assert_eq!(store.audit_chain_key().expect("audit_chain_key"), new_audit);
        // Audit chain key has previous slot now; server_secret previous still
        // None because we haven't rotated it yet.
        store
            .rotate_server_secret(new_secret, 3_000)
            .expect("rotate_server_secret");
    }

    // Step 5: store_identity_after_rotate + crash.
    let cur_seed = [0x04u8; 32];
    let prev_seed = [0x05u8; 32];
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-4");
        assert_eq!(
            store.server_secrets().expect("secrets").server_secret,
            new_secret
        );
        // lockout_secret never moves under server_secret rotation.
        assert_ne!(
            store.server_secrets().expect("secrets").lockout_secret,
            new_secret
        );

        store
            .store_identity_after_rotate(cur_seed, prev_seed, 4_000, 7)
            .expect("store_identity_after_rotate");
    }

    // Step 6: store_audit_checkpoint + crash.
    let chk_seq = 99u64;
    let chk_hmac = [0x06u8; 32];
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-5");
        let identity = store.identity_state().expect("identity_state");
        assert_eq!(identity.current_version(), 7);
        assert_eq!(identity.rotation_until_ns(), Some(4_000));
        assert!(identity.previous_pub().is_some());

        store
            .store_audit_checkpoint(chk_seq, chk_hmac)
            .expect("store_audit_checkpoint");
    }

    // Final reopen — every rotation persisted, none lost.
    let store = ServerMetaStore::open_or_init(&path).expect("reopen-final");

    // Ticket: rotated once.
    assert_eq!(store.ticket_keys().expect("ticket_keys").0, new_ticket);
    assert_eq!(
        store.ticket_keys().expect("ticket_keys").1,
        Some(initial_ticket)
    );

    // Audit chain: rotated once.
    assert_eq!(store.audit_chain_key().expect("audit_chain_key"), new_audit);

    // Server secret: rotated once.
    assert_eq!(
        store.server_secrets().expect("secrets").server_secret,
        new_secret
    );

    // Identity: stored.
    let identity_final = store.identity_state().expect("identity_state");
    assert_eq!(identity_final.current_version(), 7);
    assert_eq!(identity_final.rotation_until_ns(), Some(4_000));
    assert!(identity_final.previous_pub().is_some());

    // Audit checkpoint: stored.
    assert_eq!(store.audit_checkpoint(), Some((chk_seq, chk_hmac)));

    // Sanity: nothing leaked or zeroed during the crash sequence.
    assert_ne!(initial_audit, new_audit);
    assert_ne!(initial_secret, new_secret);
    assert_ne!(initial_ticket, new_ticket);
}

// ---------------------------------------------------------------------------
// Debug impl never leaks key bytes.
// ---------------------------------------------------------------------------

#[test]
fn debug_impl_redacts_all_secrets() {
    let (_dir, path) = tmp_path();
    let store = ServerMetaStore::open_or_init(&path).expect("init");
    let dbg = format!("{:?}", store);
    assert!(
        dbg.contains("<REDACTED>"),
        "Debug must contain redaction marker, got: {dbg}"
    );
    assert!(
        !dbg.contains("0x"),
        "Debug must not include raw byte literals: {dbg}"
    );
}

// ---------------------------------------------------------------------------
// MetaError is `Send + Sync + 'static` (so it composes with anyhow + tokio).
// ---------------------------------------------------------------------------

#[test]
fn meta_error_bounds() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<MetaError>();
}

// ---------------------------------------------------------------------------
// Lockout snapshot round-trip (M2 — persistent lockout store).
// ---------------------------------------------------------------------------

#[test]
fn lockout_snapshot_round_trips_through_reopen() {
    use shamir_connect::server::lockout::{
        FailureState, LockoutSnapshot, LockoutState, PairKey, Subnet,
    };

    let (_dir, path) = tmp_path();

    // Fresh store has no snapshot yet.
    {
        let store = ServerMetaStore::open_or_init(&path).expect("init");
        assert!(
            store
                .lockout_snapshot()
                .expect("lockout_snapshot read")
                .is_none(),
            "fresh store: lockout_snapshot must be None",
        );
    }

    // Write a snapshot, drop store (simulated crash), then reopen.
    let key_1: PairKey = (Subnet::V4([10, 0, 1]), [0xaau8; 16]);
    let key_2: PairKey = (
        Subnet::V6([0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 1]),
        [0xbbu8; 16],
    );
    let snap = LockoutSnapshot {
        failures: vec![(
            key_1,
            FailureState {
                count: 3,
                last_fail_at_ns: 1_700_000_000_000_000_000,
            },
        )],
        lockouts: vec![(
            key_2,
            LockoutState {
                triggered_at_ns: 1_700_000_000_000_000_000,
                duration_ns: 3_600_000_000_000,
            },
        )],
        total_lockouts: 7,
        captured_at_ns: 1_700_000_000_000_000_000,
    };

    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-1");
        store
            .store_lockout_snapshot(&snap)
            .expect("store_lockout_snapshot");
    }

    // Reopen → snapshot must survive verbatim (msgpack round-trip).
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-2");
        let loaded = store
            .lockout_snapshot()
            .expect("lockout_snapshot read")
            .expect("snapshot must be present");
        assert_eq!(loaded.failures.len(), 1);
        assert_eq!(loaded.failures[0].0, key_1);
        assert_eq!(loaded.failures[0].1.count, 3);
        assert_eq!(
            loaded.failures[0].1.last_fail_at_ns,
            1_700_000_000_000_000_000
        );
        assert_eq!(loaded.lockouts.len(), 1);
        assert_eq!(loaded.lockouts[0].0, key_2);
        assert_eq!(loaded.total_lockouts, 7);
        assert_eq!(loaded.captured_at_ns, 1_700_000_000_000_000_000);
    }

    // Overwrite with a fresher snapshot → second value wins.
    let snap2 = LockoutSnapshot {
        failures: vec![],
        lockouts: vec![],
        total_lockouts: 99,
        captured_at_ns: 1_800_000_000_000_000_000,
    };
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-3");
        store
            .store_lockout_snapshot(&snap2)
            .expect("store_lockout_snapshot #2");
    }
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-4");
        let loaded = store
            .lockout_snapshot()
            .expect("lockout_snapshot read")
            .expect("snapshot must be present");
        assert_eq!(loaded.total_lockouts, 99);
        assert!(loaded.failures.is_empty());
        assert!(loaded.lockouts.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Rate-limit snapshot round-trip (M2 follow-up — persistent rate limiter).
// ---------------------------------------------------------------------------

#[test]
fn ratelimit_snapshot_round_trips_through_reopen() {
    use shamir_connect::server::lockout::Subnet;
    use shamir_connect::server::rate_limit::{
        InMemoryRateLimiter, RateDecision, RateLimiter, WARMUP_WINDOW_NS,
    };

    let (_dir, path) = tmp_path();

    // Fresh store has no rate-limit snapshot yet.
    {
        let store = ServerMetaStore::open_or_init(&path).expect("init");
        assert!(
            store
                .ratelimit_snapshot()
                .expect("ratelimit_snapshot read")
                .is_none(),
            "fresh store: ratelimit_snapshot must be None",
        );
    }

    // Drain a subnet's bucket, snapshot it, and store via the meta store.
    // Anchor at real wall-clock so the snapshot's `captured_at_ns`
    // (stamped with `UnixNanos::now()`) shares a clock with the bucket
    // timestamps — matching production, where the idle-GC check on restore
    // must not discard a freshly-drained bucket.
    let subnet = Subnet::V4([10, 0, 1]);
    let boot = shamir_connect::common::time::UnixNanos::now().as_u64();
    let now = boot + WARMUP_WINDOW_NS + 1; // past warmup → 10/sec
    let snap = {
        let limiter = InMemoryRateLimiter::new(boot);
        for _ in 0..10 {
            assert_eq!(limiter.check(subnet, now), RateDecision::Allowed);
        }
        // 11th throttled → bucket drained.
        assert!(matches!(
            limiter.check(subnet, now),
            RateDecision::RateLimited { .. }
        ));
        limiter.snapshot()
    };
    assert_eq!(snap.buckets.len(), 1);

    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-1");
        store
            .store_ratelimit_snapshot(&snap)
            .expect("store_ratelimit_snapshot");
    }

    // Reopen → snapshot survives the msgpack round-trip, and rehydrating
    // it into a fresh limiter keeps the drained subnet throttled (no free
    // refill across the simulated downtime).
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-2");
        let loaded = store
            .ratelimit_snapshot()
            .expect("ratelimit_snapshot read")
            .expect("snapshot must be present");
        assert_eq!(loaded.buckets.len(), 1);

        let boot_at = now + 3600 * 1_000_000_000; // long downtime
        let limiter = InMemoryRateLimiter::with_snapshot(loaded, boot_at);
        assert_eq!(limiter.tracked_subnets(), 1);
        // At the boot instant the depleted bucket stays throttled — the
        // downtime granted no free refill (refill clock re-anchored to
        // boot).
        assert!(
            matches!(
                limiter.check(subnet, boot_at),
                RateDecision::RateLimited { .. }
            ),
            "restored drained bucket must not refill across downtime",
        );
    }

    // Overwrite with a fresher (empty) snapshot → second value wins.
    let snap2 = {
        let limiter = InMemoryRateLimiter::new(0);
        limiter.snapshot()
    };
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-3");
        store
            .store_ratelimit_snapshot(&snap2)
            .expect("store_ratelimit_snapshot #2");
    }
    {
        let store = ServerMetaStore::open_or_init(&path).expect("reopen-4");
        let loaded = store
            .ratelimit_snapshot()
            .expect("ratelimit_snapshot read")
            .expect("snapshot must be present");
        assert!(loaded.buckets.is_empty());
    }
}

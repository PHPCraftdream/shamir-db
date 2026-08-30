# shamir-connect — Cross-Lens Review (all 7 lenses, synthesized)

Crate: `crates/shamir-connect/` — the ShamirDB connection-protocol library: SCRAM-Argon2id
authentication, Ed25519 server identity (TOFU pinning + rotation), ticket-based session
resumption, per-subnet lockout, post-auth rate limiting, audit chaining, and the matching
client flows, shared by `shamir-server`, the transports, and external embedders.

Review basis: the seven 2026-08-14 lens reports under
`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-connect/` —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`, `performance-hotpath.md`,
`api-wire-protocol.md`, `error-handling-lifecycle.md`, `style-claude-md.md` — synthesized
(read-only) into this one file. Structure/tone calibrated on the two exemplar syntheses:
`shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`. The workspace-wide
`SUMMARY.md` rows for this crate (76 lens-tagged findings, "needs focused remediation —
security-gate asymmetries, rate-limit race, unbounded audit memory") were consulted as context
only. Key file:line references were spot-checked against the current source; no build/test/lint
was run and no source file was modified.

## Executive summary

The crate's crypto core is genuinely disciplined (constant-time SCRAM proofs, unconditional
anti-enumeration fake material, `verify_strict` Ed25519, zero `unsafe`, strong pinned wire
vectors), but it is **not shippable as-is**: (1) **the post-auth rate limiter is defeatable
multiplicatively under concurrency** — `Session::PostAuthBucket` prices refill off the value
returned by `last_refill_at_ns.fetch_max`, so k simultaneous callers all credit the same
elapsed span and one "session" sustains ~64× its nominal rate (the #1090 class the project
already fixed once, at a sibling site that still has the plain-store variant); (2) **the TOFU
pin callback fires before Ed25519 identity verification completes**, so an active MITM can get
its key pinned on a handshake that then fails — persistent MITM on every later connect; (3) **the
two public dispatch entry points are not the "functionally identical" twins the doc claims** —
the exported owning `dispatch_request` silently skips the #608 post-auth rate gate its `_view`
sibling enforces. Fix those three first, then the per-auth `cap_lock` that scans *all* sessions
under one global mutex on every login, `AuditChain`'s unbounded in-memory Vec (an OOM-by-design),
and the durable replay counter that bricks ticket families on a transient fsync error with zero
logging.

---

## 1. correctness-tdd

### 1.1 — high — TOFU pin callback fires before Ed25519 identity verification completes
- File:line: `crates/shamir-connect/src/client/handshake.rs:264-294` (`ClientHandshake::process_auth_ok`; verified: `pin_callback` at `:274` precedes `verify_identity` at `:288`).
- Issue: the TOFU branch invokes `pin_callback(&received_hash)` *before* `verify_identity` runs. The doc contract says the callback fires "exactly when this is the first connection to this host (TOFU): caller decides whether to persist the pin" — and `HandshakeSuccess` carries no pin hash, so persisting inside the callback is the only way a caller can implement TOFU. If any later check fails, `process_auth_ok` returns `Err` ("caller MUST disconnect") but the callback has already handed out the (possibly hostile) key hash.
- Failure scenario: an active MITM on first connect proxies a real SCRAM exchange to the legitimate server (so `server_signature` verifies) and substitutes its own `server_pub_key` in `auth_ok`; the callback fires with SHA256(attacker pub) and a naive caller persists it; the strict verify then fails and the handshake errors — but the attacker's key is now pinned. On every subsequent connection the attacker (who owns that keypair and can sign `identity_input`) passes both checks while proxying SCRAM: persistent MITM. Test gap: `tofu_first_connect_invokes_pin_callback` (valid sig) and `tampered_identity_sig_aborts_client` (pin set) exist, but the TOFU + failed-verify combination — the ordering hazard itself — is untested.
- Fix: run `verify_identity` before the TOFU branch (or defer the callback to immediately before `Ok(HandshakeSuccess)`). Red test: TOFU handshake with tampered `identity_sig` must return `Err(ServerSignatureInvalid)` **and** must not have invoked `pin_callback`.

### 1.2 — medium — Per-subnet rate-limiter refill watermark can regress — the exact #1090 hazard, unfixed at the sibling site
- File:line: `crates/shamir-connect/src/server/rate_limit.rs:337-356` (`InMemoryRateLimiter::check`; verified: `:342` `b.last_refill_at_ns = now_ns;`).
- Issue: same-key callers serialize on the DashMap entry write-lock, but each caller's `now_ns` was captured *before* reaching the map. A thread preempted between reading the clock and acquiring the entry stores an older watermark over a newer one (the store is unconditional), so the next `check` computes `elapsed` against a regressed point and re-credits an already-credited wall-clock interval. This is precisely the hazard fixed in `Session::PostAuthBucket` via `fetch_max` (#1090, 2026-08-11) and documented as "unbounded over-refill" in `session.rs` — but the per-subnet `BucketState` site kept the plain store. Sibling of finding 2.1 (same race class, different site/mechanism — `fetch_max` fixes regression there but still loses to the racer-count multiplication).
- Failure scenario: a burst of concurrent `auth_init` from one subnet (the exact scenario the limiter exists for) queues threads on the shard lock; the one holding the oldest `now_ns` commits last, regressing the watermark by its queue time; subsequent checks grant `rate × regression` free tokens. TDD gap: `out_of_order_now_ns_credits_no_extra_tokens` exists only for `check_post_auth_rate_limit`; no equivalent pins this site.
- Fix: `b.last_refill_at_ns = b.last_refill_at_ns.max(now_ns)` (compute `elapsed` against the pre-max value) and port the out-of-order-`now_ns` test to `rate_limit_tests.rs`.

### 1.3 — medium — `verify_against_checkpoint` flags a legitimately stale checkpoint as truncation
- File:line: `crates/shamir-connect/src/server/audit_chain.rs:267-292`.
- Issue: the check rejects unless `checkpoint_seq ∈ {last_seq, last_seq + 1}`. But checkpoints are persisted only periodically (every 60 s / 1000 events per the module's own docs and `AuditChainWriter::append`), so at restart the loaded log almost always contains entries appended *after* the last checkpoint — `checkpoint_seq < last_seq` is the healthy steady state, not truncation. The doc defines truncation only as "checkpoint is ahead of the chain", yet the code also fires when the checkpoint is *behind* by more than one entry.
- Failure scenario: checkpoint written at `next_seq = 101`; 50 more entries (101–150) appended; crash. On restart, `verify_against_checkpoint(log, 101, …)` returns `Err(TruncationDetected { checkpoint_seq: 101, final_seq: 150 })` although nothing was truncated — a startup false alarm. `truncation_defence_*` tests and `shamir-server/tests/audit_appender.rs` exercise only exactly-aligned checkpoints. (Currently no production call site wires this verifier — dormant, but wrong as specified.)
- Fix: truncation ⇔ `checkpoint_seq > last_seq + 1`; require `last_hmac == checkpoint_hmac` only when `checkpoint_seq == last_seq + 1`. Add a red test with `checkpoint_seq << last_seq` expecting `Ok`.

### 1.4 — medium — *(primary: same defect as 5.1)* `client` feature cannot build without `server`
- Full write-up at 5.1 (api-wire is the primary lens; rated high there). The correctness lens contributes: `Cargo.toml:17-19` declares independent features, every client module ungated-imports `crate::server::*`, and `cargo check -p shamir-connect --no-default-features --features client` fails with E0433 — a manifest-supported combination.

### 1.5 — medium — *(primary: same defect as 7.1)* `dispatch_request` omits the post-auth rate-limit gate its "functionally identical" twin has
- Full write-up at 7.1 (style is the primary lens; rated high there). The correctness lens contributes: the divergence is untested — no test asserts rate limiting on either entry point via `dispatch_request`.

### 1.6 — low — *(primary: same defect as 2.7)* `ServerIdentityState::rotate` / `try_finalize` are load-clone-store races despite the "Atomic:" doc
- Full write-up at 2.7. The correctness lens contributes the mirror/inner divergence detail: a stale-snapshot `try_finalize` landing after `rotate` stores `previous: None, rotation_until: None` with the *old* keypair while `current_version_atomic` keeps the incremented version.

### 1.7 — low — *(primary: same defect as 6.2)* Password buffers are not zeroized on early-error returns
- Full write-up at 6.2. Reachability confirmed independently by this lens: `validate_client_kdf_safe` rejects over-cap params → early `?` return before `password.zeroize()` (`client/handshake.rs:202-238`, zeroize only at `:232`).

### 1.8 — low — *(primary: same defect as 6.3)* changePassword TTL check underflows on clock regression; TTL boundary untested
- Full write-up at 6.3.

### 1.9 — low — *(primary: same defect as 5.3)* `encode_details_canonical` is a placeholder that encodes nothing
- Full write-up at 5.3.

### 1.10 — low — Vacuous variant assertions in the all-zero nonce tests
- File:line: `crates/shamir-connect/src/common/tests/auth_message_tests.rs:151` and `:175`.
- Issue: `matches!(err, crate::common::Error::InvalidInput(_));` — the `matches!` result is evaluated and **discarded** (statement, not `assert!`). The two tests (`rejects_all_zero_client_nonce`, `rejects_all_zero_server_nonce`) only assert "an error occurred" via `unwrap_err`; the variant check is dead code. A refactor returning `Error::InvalidUsername` for these paths would pass the suite — the assertion that would catch the regression doesn't assert.
- Fix: `assert!(matches!(err, Error::InvalidInput(_)));` on both lines (grep confirms these are the only two occurrences in the crate).

### 1.11 — low — `InMemoryRateLimiter::with_rate(_, 0)` breaks the limiter after warmup
- File:line: `crates/shamir-connect/src/server/rate_limit.rs:186-192, 329-367`.
- Issue: `rate_per_sec = 0` passes through unvalidated. During warmup `effective_rate_per_sec` clamps to ≥ 1, but after warmup it returns 0: `capacity_at_rate(0) == 0`, so the first request's `or_insert_with` computes `capacity - cost = 0 − 1e9` (u64 underflow — debug panic, release wraps to ~u64::MAX, i.e. a silently unlimited bucket), and the throttle branch divides by `rate as u64` (division by zero).
- Fix: validate `rate_per_sec >= 1` in `with_rate`/`with_snapshot_sink_and_rate` (return or clamp), or make `effective_rate_per_sec` `.max(1)` unconditionally.

### 1.12 — nit — `Vec::with_capacity` constant off by 2 in `AuthMessage::build`
- File:line: `src/common/auth_message.rs:82` (dup-flagged by api lens nit, folded here).
- Issue: fixed fields sum to 144 (14+2+32+32+16+4+4+4+1+1+1+32+1), not 142 — every `build` does one guaranteed realloc; the module doc's "149 total for a 5-byte username" is consistent with 144. Fix the constant (or compute once as a `const`).

### 1.13 — nit — Unused `unicode-normalization` dependency
- File:line: `Cargo.toml:40` (workspace `Cargo.toml` dependency block). NFC is applied inside `precis-profiles::enforce`; remove the dep or annotate why it is kept.

### 1.14 — nit — Ticket ciphertext length silently truncates at `u16`
- File:line: `src/server/ticket.rs:118`. `(self.ciphertext.len() as u16)` truncates ciphertexts > 64 KiB; `from_bytes` would then reject the corrupt ticket. Unreachable with current ~150-byte plaintexts, but a `debug_assert!`/error beats silent truncation.

### 1.15 — nit — Undocumented threshold asymmetry in `client/rotation.rs`
- File:line: `src/client/rotation.rs:63` vs `:135`. The broadcast-event path requires `transition_until_ns > now + 60 s` while orphan-recovery requires only `> now`. Probably intentional (event-vs-auth_ok freshness), but undocumented.

### 1.16 — nit — (folded bundle items, deduped to their primary lenses)
Four items from this lens's nit bundle are the same defects flagged elsewhere: `auth_message.rs:6` stale `test-vectors/auth_v1/` path → **7.5**; `kdf_params.rs:77` `Result<(), String>` → **5.8**; `crypto.rs:66-70, 215-222` / `time.rs:18-23` `.expect` panics in library code → **6.6**; `server/handshake.rs:232` dead `let _ = constant_time_eq;` suppressor → **7.4**.

---

## 2. concurrency-lockfree

### 2.1 — high — **HEADLINE: concurrent callers share the pre-`fetch_max` refill watermark and multiply refill by racer count — the documented invariant does not hold**
- File:line: `crates/shamir-connect/src/server/session.rs:353-358` (verified: `fetch_max` at `:356`; claims at `session.rs:198-215` and `337-352`).
- Issue: `check_post_auth_rate_limit` computes `elapsed = now_ns - prev_refill_ns` from the value returned by `last_refill_at_ns.fetch_max(now_ns)`. For a *sequential* call sequence this telescopes and total refill is bounded by wall-clock span, exactly as the doc claims. But k *concurrent* callers that all reach `fetch_max` before any of their stores commit all observe the *same* pre-existing watermark and each independently computes `elapsed = now - watermark` — the same span is credited k times. The debit side (`micro_tokens` CAS) is exact; the refill side is not. The doc's security-invariant claim ("total refill across any sequence of calls is bounded by the true wall-clock span") is therefore false under concurrency, and the two-atomics design cannot express "refill + watermark advance + debit" as one linearization point. This is the same race class as 1.2 (fixed there as #1090 via `fetch_max`, but `fetch_max` alone only prevents watermark *regression*, not the racer-count multiplication).
- Failure scenario: rate = 500/s, burst capacity 500. Attacker holds one bearer token over 64 connections (`dispatch_request_view` gates per request; the same `session_id` is shared across connections). Drain the bucket at t0. At t0+100ms fire 64 simultaneous requests: each racer reads watermark t0, credits 100ms×500/s = 50 tokens, debits 1 → all 64 admit and ~2.4k tokens of surplus remain. Repeat batches every 100ms → sustained ≈ 64 × 500 req/s from one "session". The #1090 concurrency test (`server/tests/post_auth_rate_limit_tests.rs:97-131`) races all callers at `now_ns == watermark` so `elapsed == 0` for every racer — it pins debit atomicity but is structurally blind to this refill race. Sub-note (deduped to 7.1): `dispatch_request` (`server/dispatch.rs:69-110`) never calls `check_post_auth_rate_limit` — only `dispatch_request_view` does.
- Fix: make refill and watermark advance part of the same atomic op — pack `(watermark, tokens)` into one `AtomicU64` and do refill+debit+advance in a single `fetch_update` CAS loop (quantize the watermark, e.g. µs/ms resolution; re-derive `retry_after` from the packed state). Alternatively keep two atomics but load the watermark *inside* the `fetch_update` closure so each committed attempt prices refill off the freshest watermark (residual multi-credit shrinks to the CAS window), and correct the doc. Regression test must race callers under a `Barrier` with `now > watermark` — the existing single-instant test cannot catch this.

### 2.2 — high — `SessionStore::cap_lock`: unjustified `parking_lot::Mutex` on the per-auth hot path, held across an O(all-sessions) full-store scan *(also flagged as [high] by performance lens 4.1)*
- File:line: `crates/shamir-connect/src/server/session.rs:416` (lock, verified), `470-493` (critical section, `let _cap_guard` at `:470`).
- Issue: CLAUDE.md bans `parking_lot::*` in hot paths and requires every use justified inline with a contention-model comment; `cap_lock` has no such comment anywhere (struct field, `new`, or `insert_with_per_user_cap`). Worse, the critical section iterates the **entire** `by_sid` DashMap to collect the inserting user's sessions (`for entry in self.by_sid.iter()`) — O(total sessions), not O(sessions of that user) — under a lock that serializes session creation for **all users**. This violates pillar 1 (lock on hot path) and pillar 3 (hidden O(N) per op, no ack annotation). `insert_with_per_user_cap` runs on every successful SCRAM auth (`shamir-server/src/connection/handshake.rs:579`). Coverage: `tests/integration_session.rs:381-448` is single-threaded only; no concurrent-insert test.
- Failure scenario: server at 100k live sessions; a login storm (e.g. after a partition heals) funnels every `auth_ok` through one mutex; each holder scans 100k entries for its ≤16 sessions while every other completing handshake blocks behind it — auth latency climbs with unrelated session population, and the lock converts a sharded concurrent map into a serialized one exactly when load is highest. Reconcile-the-cap sub-note (deduped to 4.3): `process_resume` (`server/resume.rs:432`) uses the uncapped `insert`, so resumed sessions never count against `MAX_SESSIONS_PER_USER`.
- Fix: per-user secondary index (`DashMap<[u8;16], Vec<[u8; SESSION_ID_BYTES]>, FxBuild>` updated in insert/remove/kick/GC, or `scc::HashMap<user_id, tiny sid set>`), making eviction O(≤cap) and the lock per-user granularity. If a global lock is kept deliberately, add the mandated contention-model comment and the `// O(N) ack:` annotation. The style lens (7.3) independently flags the missing contention-model comment; performance lens 4.1 is the same defect in O(x→0) framing.

### 2.3 — medium — `AuditChain::inner`: hot-path `parking_lot::Mutex` with no contention-model comment, critical section contains HMAC compute, string allocations, and a full entry clone *(also flagged as [low] by performance lens 4.4)*
- File:line: `crates/shamir-connect/src/server/audit_chain.rs:131` (field), `196-215` (`append` critical section).
- Issue: `append` fires per audit event — per auth attempt/failure, per eviction, per admin op — request-rate frequency, not setup-only. Per CLAUDE.md a hot-path `parking_lot::Mutex` must carry an inline contention-model justification; the doc only describes the layout. Inside the lock the code allocates up to five `String`s, computes the entry HMAC (SHA-256 over canonical bytes), then clones the whole entry into the in-memory vec — hold time is dominated by heap churn + crypto, not by the seq/prev update that actually needs exclusivity. The lock is arguably defensible (seq N+1's `prev_hmac` is seq N's `hmac` — a genuine linearization dependency), but neither the justification nor a minimized CS exists. Pillar-2 exposure in the same path (sub-note): `AuditChainWriter::append` (`audit_chain.rs:427`) invokes the sync `AuditAppender::append_entry` — implementations may write sqlite + fsync — inline on the request thread; no async wrapper or documented `spawn_blocking` contract (related, but a distinct defect: 6.5).
- Failure scenario: under an auth flood each failure emits an audit event; concurrent appends serialize behind HMAC+allocation-sized critical sections; the audit mutex becomes a global throttle on the very path (auth) whose availability the spec's other defenses protect.
- Fix: shrink the CS to a reserve/publish scheme (`AtomicU64` seq + `ArcSwap` published-hmac; HMAC, string materialization, and clone outside the lock) or keep the lock with the mandated inline contention-model comment; store `Arc<AuditEntry>` so the vec push is a refcount.

### 2.4 — medium — `FjallConsumedCounters` doc claims the lock is "Not held across the fsync" — the code holds it across `persist(SyncAll)`
- File:line: `crates/shamir-connect/src/server/durable_counters.rs:51-53` (claim) vs `124-149` (guard spans `get` → `insert` → `persist(PersistMode::SyncAll)`).
- Issue: the `write_lock` field doc states "Not held across the fsync — fjall's `persist` is synchronous and short." The `_guard` acquired at line 124 lives to end of scope, so the `SyncAll` fsync at line 147 — typically milliseconds of disk latency — runs while holding the lock. The contention model itself is properly named and sound ("one call per session resumption"), so this is doc/code drift, not a wrong design — but these inline contention-model comments are the crate's enforcement mechanism per CLAUDE.md, and a future reader tuning resumption concurrency will trust the false claim.
- Failure scenario: none today beyond misdocumented serialization of concurrent resumes' fsyncs; risk is future code built on the incorrect statement.
- Fix: correct the comment to say the lock *is* held across the (deliberately serialized) fsync, or release before persist if the get→insert→persist ordering permits.

### 2.5 — low — `Argon2Semaphore` exposes a blocking `Mutex`+`Condvar` wait that no production caller uses — an executor-parking trap if adopted
- File:line: `crates/shamir-connect/src/server/argon2_semaphore.rs:20-21, 29-38, 84-110`.
- Issue: `acquire`/`acquire_until` block the calling thread (std `Mutex`+`Condvar` wait loop) and the module doc presents this as the design. The only production consumer (`shamir-server`) exclusively uses `try_acquire` and routes real Argon2id through `spawn_blocking`; the blocking API has zero production callers (tests only). If a future caller invokes `acquire_until` from an async auth task once 64 permits are exhausted, tokio workers park on the condvar — a pillar-2 violation with the classic "SLOW/TIMEOUT under load" symptom. (Style lens 7.3 additionally flags the missing sanctioned-category comment for the `std::sync::Mutex`+`Condvar`.)
- Failure scenario: an integrator reads the module doc, calls `sem.acquire()` before Argon2 in an async handler; at 64 concurrent derivations every worker thread blocks for up to the Argon2 duration → runtime starvation, request timeouts with no panic to point at.
- Fix: document in bold on `acquire`/`acquire_until` ("sync context / `spawn_blocking` only — never call from an async task"), rename them `acquire_blocking`, or delete the blocking surface until a caller needs it.

### 2.6 — low — `lockout.rs` DashMaps use the default `RandomState` hasher — pillar-4 drift in the one file that keys on attacker-chosen subnets
- File:line: `crates/shamir-connect/src/server/lockout.rs:256-257` (verified: plain `DashMap<PairKey, _>`).
- Issue: every other concurrent map in this crate aliases `BuildHasherDefault<rustc_hash::FxHasher>` with a justification comment — `session.rs:410`, `rate_limit.rs:155`, `resume.rs:29`, `admin.rs:21` — but `InMemoryLockoutStore::failures`/`lockouts` inherit SipHash/`RandomState`. `PairKey = (Subnet, [u8;16])` where `Subnet` derives from the client-supplied IP. Practical DoS impact is small (SipHash is keyed per-process), but the normative default is violated in exactly the module whose sibling already wrote the comment to copy.
- Failure scenario: none functional; 2–5× slower lookups on the failed-auth path and inconsistency with the documented workspace standard.
- Fix: `type PairHasher = std::hash::BuildHasherDefault<rustc_hash::FxHasher>;` on both maps, with the same DoS-rationale comment used in `rate_limit.rs`.

### 2.7 — low — `ServerIdentityState::rotate` / `try_finalize` are non-atomic check-then-`store` on `ArcSwap` — concurrent rotates lose an update and strand the interim keypair *(also flagged [low] by correctness 1.6 and security 3.4)*
- File:line: `crates/shamir-connect/src/server/rotation.rs:151-180` (`rotate`; verified: load→overlap-check→build→`store`, doc claims "Atomic:"), `184-199` (`try_finalize`).
- Issue: both do `let current = (**self.inner.load()).clone(); … self.inner.store(Arc::new(new_inner));` — a read-check-write with no CAS, while `rotate`'s doc claims "Atomic: previous = current; current = new; …". `ArcSwap` makes each individual load/store atomic, not the sequence. Two concurrent `rotate()` calls both pass the overlap pre-check against the same snapshot and both store: the first rotated keypair is silently discarded, and any `identity_sig`/ticket issued via `sign_with_current` inside the two-`store` window pins a key that is neither `current` nor `previous` afterward. A stale-snapshot `try_finalize` (background GC) landing after `rotate` stores `previous: None, rotation_until: None` with the *old* keypair while `current_version_atomic` keeps the incremented version (mirror/inner divergence), after which `is_ticket_version_acceptable` (consulted at `resume.rs:290`) compares tickets against a version the live keypair no longer carries. `try_finalize` alone is idempotent; only the `rotate` interleavings matter. Admin-only frequency caps severity at low.
- Failure scenario: (a) double-invoked rotation (retry + original racing) yields clients that pinned the discarded keypair; their next handshake fails pin verification and the `rotation_in_progress` recovery payload is signed by the *original* previous key — orphan recovery breaks for that cohort. (b) rotate racing the finalize sweep breaks overlap-window guarantees and can reject every ticket-based resume (self-DoS) until the next rotation.
- Fix: `ArcSwap::compare_exchange`/`rcu` loop re-checking the overlap precondition on the observed `Arc` after building `new_inner` (or the sanctioned rare-admin `parking_lot::Mutex` with an inline contention comment); make `try_finalize` refresh the atomic mirror from the same committed snapshot; add a rotate/finalize interleaving test.

### 2.8 — nit — Stale `Debug` label reports `permissions` as `"<RwLock>"`
- File:line: `crates/shamir-connect/src/server/session.rs:111`. The field has been a plain `SessionPermissions` since the `parking_lot::RwLock` was removed (see the field's own doc at lines 143-148). Change the label to `"<snapshot>"` or drop the field from the impl.

---

## 3. security-crypto

### 3.1 — medium — *(primary: same defect as 7.1)* Public `dispatch_request` silently skips the post-auth rate limit that `dispatch_request_view` enforces
- Full write-up at 7.1. Security framing: `dispatch_request` is publicly re-exported at `server/mod.rs:28` as a peer API; any embedder/transport binding picking the owning variant loses the task-#608 per-session flood control with no error, warning, or type-level distinction. An authenticated client drives unbounded request-rate; handler/DB resources are exhausted.

### 3.2 — low — Long-lived server secrets are plain `[u8; 32]`, outside the crate's own zeroization policy
- File:line: `crates/shamir-connect/src/server/config.rs:29-31` (`server_secret`, `lockout_secret`); `crates/shamir-connect/src/server/resume.rs:130-131` (`ticket_key`, `ticket_key_previous`).
- Issue: `common/crypto.rs`'s contract says the layer "enforces zeroization on key material (`Zeroizing<[u8; 32]>`)" and all SCRAM-derived values comply — but the crown-jewel long-lived secrets (anti-enumeration HKDF IKM, lockout HMAC key, ticket AES-GCM keys) are bare arrays: they clone freely (`ServerSecrets: Clone`, `ResumeConfig` fields, `issue_initial_ticket(&[u8; 32])`) and are never wiped on drop, unlike everything derived from them.
- Failure scenario: a core dump, heap-swap inspection, or future `Debug`/serialization path recovers `server_secret` indefinitely, defeating the zeroization discipline applied to `salted_password`/`client_key`/`server_key`.
- Fix: wrap in `Zeroizing<[u8; 32]>` (derive `Clone` only), keeping `&[u8]` views for internal use; `ResumeConfig` retains pre-scheduled ciphers plus zeroizing key copies.

### 3.3 — low — *(primary: same defect as 6.2)* Client password buffers not zeroized on early-error paths
- Full write-up at 6.2. This lens's file:line detail: `client/handshake.rs:202-232` (doc at `:201` promises "password is consumed and zeroized on return"; early returns at 209-215 precede the zeroize), `client/bootstrap.rs:85-97`, `client/changepw.rs:24-72` (both `old_password` and `new_password`).

### 3.4 — low — *(primary: same defect as 2.7)* `ServerIdentityState::rotate` / `try_finalize` non-atomic check-then-act
- Full write-up at 2.7. Security framing: after a broken interleaving, `is_ticket_version_acceptable` compares tickets against a version the live keypair no longer carries — every ticket-based resume rejected until the next successful rotation.

### 3.5 — low — Known-user challenge exposes per-user KDF params — residual enumeration channel for users below current defaults (accepted trade-off per spec §13.5; recorded so the decision stays visible)
- File:line: `crates/shamir-connect/src/server/handshake.rs:154-159` (`effective_kdf` selection), `174-185` (`ChallengeView`).
- Issue: `challenge()` returns the real user's stored `kdf_params` (plus salt) for known users and server defaults for unknown ones, and Argon2id wall-time scales with those params. After a server-wide KDF-default bump (spec §13 upgrade flow), every not-yet-upgraded user is distinguishable from "unknown user" by one challenge field — or by timing, since the 50-75 ms padding floor (`common/latency.rs`) cannot mask multi-hundred-ms Argon2id deltas (19 MB/t2 vs 128 MB/t4).
- Failure scenario: targeted username enumeration of legacy-parameter accounts following a defaults bump.
- Fix: none required if spec §13.5 consciously accepts this; otherwise pad the Argon2id phase to the params-independent worst case and document that callers must size `FIXED_FLOOR_MS` from the server's KDF *minimum*, not its defaults.

### 3.6 — nit — `start_change_password_challenge` accepts an all-zero `client_nonce_cp`
- File:line: `crates/shamir-connect/src/server/changepw.rs:64-90`; the all-zero rejection happens only later inside `build_auth_message_cp` (`crates/shamir-connect/src/common/changepw.rs:59-64`).
- Issue: a client submitting a zero nonce gets a pending challenge stored and a `challenge_cp` issued, then deterministically fails at verify — asymmetric with `ServerHandshake::new`, which rejects all-zero nonces at issuance. No replay impact (both nonces are server-stored and single-use).
- Fix: validate `client_nonce_cp` non-zero at challenge start for symmetry and fail-fast.

### 3.7 — nit — *(primary: same defect as 5.3)* `encode_details_canonical` dead placeholder
- Full write-up at 5.3.

### 3.8 — nit — `canonical_bytes` length prefixes truncate silently at 255/65535 bytes
- File:line: `crates/shamir-connect/src/server/audit_chain.rs:102-113` (`as u8` / `as u16` casts).
- Issue: `transport`/`user`/`ip_subnet`/`result` longer than 255 bytes (or `event` > 65535) corrupt the canonical form's length prefix. The raw bytes still follow, so the HMAC remains collision-safe, but cross-language canonical re-derivation breaks and the `debug_assert_eq!` fires in debug builds.
- Fix: reject over-long fields with an error instead of casting.

---

## 4. performance-hotpath

The documented "Optim #2..#9" work is real (pre-scheduled AES ciphers in `ResumeConfig`, zero-copy `RequestEnvelopeView`/`Ref` envelopes, `OnceLock`-cached per-session HMAC key, atomics-only `PostAuthBucket`, `FxHasher` on every fixed-size-key DashMap, exact-`with_capacity` canonical builders). The lens's two genuine pillar-3 violations are 2.2 (deduped) and 4.2 below; benches never exercise the capped insert or the audit append — exactly where both blind spots live.

### 4.1 — high — *(primary: same defect as 2.2)* Per-user session-cap insert: O(total-sessions) scan under a global mutex on every login
- Full write-up at 2.2. Perf framing: spec §7.4 NORMATIVE LRU-cap enforcement costs O(total live sessions) per login and globally serializes session creation — total session-creation work is O(sessions × logins). Fix per 2.2 (per-user index; then drop `cap_lock` or narrow it to the per-user shard).

### 4.2 — high — `AuditChain` accumulates every audit event in an ever-growing in-memory Vec (unbounded growth)
- File:line: `crates/shamir-connect/src/server/audit_chain.rs:140` (`entries: Vec<AuditEntry>`, verified), `:214` (`g.entries.push(entry.clone())`, verified); `from_checkpoint` same shape at `:170-179`.
- Issue: every `append` unconditionally clones + pushes the entry into `ChainInner::entries`; no cap, no drain, no opt-out — the `AuditAppender` is an *additional* persistence sink, not a replacement, so the module doc's "production should override with a streaming writer" is not achievable via any API. Production wires exactly this (`crates/shamir-server/src/server/server_launcher.rs:339` creates the shared `AuditChain`). Each `AuditEntry` carries six `String`s, a `Vec`, and two 32-byte tags (hundreds of bytes), and audit events fire at least once per connection attempt.
- Failure scenario: a long-running server accumulates millions of entries → RSS grows monotonically forever → eventual OOM; a memory leak by design. The periodic `checkpoint()` only persists `(next_seq, prev_hmac)` and never trims.
- Fix: make the in-memory log opt-in for tests (e.g. `AuditChain::new_in_memory`), with the default chain keeping only `(next_seq, prev_hmac)` and handing entries straight to the appender; or keep a bounded ring (documented constant). If kept, store `Arc<AuditEntry>` so the push is a refcount, not a deep clone.

### 4.3 — medium — Resume path bypasses MAX_SESSIONS_PER_USER — uncapped session creation on the hottest session-creation path
- File:line: `crates/shamir-connect/src/server/resume.rs:432` (`session_store.insert(session_id, session)`).
- Issue: `process_resume` mints and inserts a fresh `Session` per resumption via plain `insert`, never calling `insert_with_per_user_cap` — the only API enforcing the §7.4 NORMATIVE per-user cap (confirmed: `shamir-server`'s `run_resume` adds no cap enforcement after the call). The cap+LRU machinery exists only on the full-auth path — and it is the O(N) one (2.2) — while the hotter reconnect path grows each user's session count unbounded between idle-GC ticks. (Also flagged as a reconcile-gap sub-note inside 2.2.)
- Failure scenario: a client (legitimate multi-tab or a replaying stolen ticket) resumes every few seconds, each resume minting a new session id; per-user entries and the global map grow until the external idle-GC task runs — amplifying 2.2's scan cost and skirting the spec cap.
- Fix: route the insert through `insert_with_per_user_cap` inside `process_resume` (cheap once 2.2's index exists) and return the evicted sid in `ResumeOk`-adjacent plumbing so callers can emit `session_evicted{reason="max_sessions_lru"}`.

### 4.4 — low — *(primary: same defect as 2.3)* `AuditChain::append` holds the chain mutex across allocations, HMAC, and a deep entry clone
- Full write-up at 2.3. Perf framing: bounded impact (~µs each) under an auth burst, hence low here; the `Arc<AuditEntry>` vec-storage alone would remove the in-lock clone.

### 4.5 — nit — Owning-envelope `dispatch_request` still pays a wall-clock syscall per request
- File:line: `crates/shamir-connect/src/server/dispatch.rs:78` (`store.lookup` → `Session::touch` → `UnixNanos::now()`, `session.rs:296-298`).
- Issue: the owning variant samples `UnixNanos::now()` (~100 ns syscall on Windows) per request via `lookup()`; the zero-copy `dispatch_request_view` (Optim #4/#5) is the documented hot path, yet the non-view variant remains exported and is benched as `dispatch/happy_path` (`benches/hot_paths.rs:234`), inviting use on hot transports. Largely subsumed if 7.1's fix deprecates/gates the owning variant.
- Fix: doc-mark `dispatch_request` as the non-hot/compat entry point, or thread a caller-captured `now_ns` through it.

*Test-coverage note (not a finding): unit coverage is thorough for the hot paths that matter; `insert_with_per_user_cap` has behavioral LRU tests but nothing at scale, and `benches/hot_paths.rs` benches neither the capped insert nor `AuditChain::append` — consistent with 2.2 and 4.2 going unnoticed.*

---

## 5. api-wire-protocol

The wire layer is in strong shape overall: canonical byte strings hand-serialized with domain-separated tags, pinned by 8 byte-exact cross-language vectors; msgpack envelopes with wire-compat tests; no `serde_json` anywhere (builder-only query rule trivially satisfied — req/res are opaque blobs). The findings below are interface-quality issues.

### 5.1 — high — `client` feature cannot build without `server` — client modules unconditionally import `crate::server::*` *(also flagged [medium] by correctness 1.4)*
- File:line: `crates/shamir-connect/Cargo.toml:16-19` (verified: `client = []`, `server = [...]` independent); `src/lib.rs:25-31`; `src/client/handshake.rs:27`; `src/client/bootstrap.rs:17-19`; `src/client/changepw.rs:11`; `src/client/rotation.rs:20`; `README.md:16-17, 23-24`.
- Issue: the README advertises `default-features = false, features = ["client"]` as a "client-only SDK (smaller binary, no server-only deps)", but every file in `src/client/` imports types from `crate::server` (`RotationInProgressPayload`, `BootstrapChallenge`/`BootstrapRequest`, `ChangePwRequest`, `IdentityRotationEvent`), and `pub mod server` is `#[cfg(feature = "server")]`-gated in `lib.rs`. Under `--no-default-features --features client` the crate fails to compile with unresolved-import errors. It only compiles today because both features are on by default.
- Failure scenario: an embedder follows the README's client-only recipe; the build breaks. Any future CI job or downstream trying a client-only build breaks.
- Fix: encode the dependency as `client = ["server"]` (and fix the README claim), or move the shared wire views into `common/` so `client` genuinely stands alone.

### 5.2 — medium — *(primary: same defect as 7.1)* `dispatch_request` lacks the post-auth rate-limit gate its documented twin enforces
- Full write-up at 7.1. Api framing: the two public entry points are not behaviorally identical; latent today (shamir-server routes through the view variant only), but the doc actively asserts equivalence, so nothing signals the trap.

### 5.3 — medium — `encode_details_canonical` is a broken placeholder public API (wrong parameter type, stub body) *(also flagged by correctness 1.9 [low], security 3.7 [nit], style 7.2 [medium])*
- File:line: `crates/shamir-connect/src/server/audit_chain.rs:355-361`.
- Issue: the doc promises "encode a `BTreeMap<String, msgpack-Value>` as canonical msgpack (lex-sorted keys) for use as `details_canonical_msgpack`", but the parameter is `&BTreeMap<String, rmp_serde::config::DefaultConfig>` — a serializer *config* type, not a value type — and the body is `let _ = map; Vec::new()`. It compiles, is `pub`, has zero callers and zero tests, and always returns an empty Vec.
- Failure scenario: a caller wiring the audit chain follows the doc, passes a real details map, and silently hashes empty `details_canonical_msgpack` into every audit-chain HMAC — the canonical-bytes contract that any second implementation must reproduce byte-identically (spec §3.3) is not actually implemented.
- Fix: implement over `&BTreeMap<String, rmp_serde::Value>` (or a typed details struct) with `rmp_serde::to_vec_named` and a round-trip/canonical-bytes test, or delete the function until a real implementation exists — a stub with this signature is a trap in a public API.

### 5.4 — medium — `RequestHandler::handle`'s `Err(String)` flows verbatim onto the wire, bypassing the crate's own error-collapsing discipline
- File:line: `src/server/dispatch.rs:29-45, 100-109, 163-172`; cf. `src/common/error.rs:1-6, 86-102` (cross-ref 6.11: `to_wire`, the collapse helper, is dead code).
- Issue: `error.rs` is explicit that anything sent to a peer collapses to a generic string (spec §14.1/§14.4), and the crate's own protocol errors use the fixed §14 vocabulary (`session_expired`, `session_invalidated`, `rate_limited`). But the central public handler contract returns `Result<Vec<u8>, String>` and `ErrorEnvelope::new(request_id, err)` transmits the handler's string with no collapsing, vocabulary check, or even a doc warning.
- Failure scenario: a handler returns `format!("{e:?}")` of an internal error; internal paths/types leak to the client in the `error` field — exactly what the privacy rules in this crate's own error module forbid.
- Fix: type the handler error as the crate `Error` (route through `to_wire` in `dispatch_request*`), or at minimum document that the string must come from the §14 vocabulary and add a test pinning the allowed set.

### 5.5 — medium — `PushEnvelope.data` is not `serde_bytes`-wrapped — wire bloat and inconsistency with every other byte field in the crate
- File:line: `src/common/push_envelope.rs:31-36`; contrast `src/common/envelope.rs:25-32` and `src/server/ticket.rs:56-66`.
- Issue: `sid`, `req`, and all `TicketPlain` byte fields use `serde_bytes` (msgpack `bin`), but `PushEnvelope.data: Option<Vec<u8>>` — documented as carrying "MessagePack-encoded records, keys, etc.", i.e. the largest payloads on the wire — serializes as a msgpack *array of integers* (~3-5× larger per byte under rmp-serde). The round-trip test only proves Rust-to-Rust consistency, so the array-vs-bin choice silently becomes part of the cross-language contract a JS client must reproduce.
- Failure scenario: a subscription delivering record payloads pays a multi-fold size penalty per push; a second implementation that naturally encodes `bin` (following the crate's own dominant pattern) produces different bytes and fails interop.
- Fix: `#[serde(with = "serde_bytes")]` on the `Option<Vec<u8>>` via a custom module (or `serde_bytes::ByteBuf`) and pin the frame with a byte-exact test.

### 5.6 — low — Ticket wire version is a magic literal with asymmetric encrypt/decrypt validation
- File:line: `src/server/ticket.rs:224-226` (decrypt rejects `!= 2`), `ticket.rs:168-193` (encrypt accepts any `plain.version`), `ticket.rs:176` (AAD binds `plain.version`); `src/server/resume.rs:262-265, 397, 467` (hard-coded `2`).
- Issue: `TicketPlain.version` is a public `u8` with no constant for the v2 value. `encrypt_ticket_with_cipher` will happily encrypt and AAD-bind any version the caller set, while every decrypt path (and `process_resume` step 2) rejects anything but `2`.
- Failure scenario: a caller constructs a ticket with `version = 3` (or a future v2 → v3 migration misses one of the four `2` literals): tickets are issued that can never be resumed, failing closed but opaquely at first resume.
- Fix: add `pub const TICKET_WIRE_VERSION: u8 = 2;`, validate `plain.version == TICKET_WIRE_VERSION` inside `encrypt_ticket_with_cipher`, and use the constant at all four sites.

### 5.7 — low — No byte-exact vectors for two signed canonical strings: `auth_message_cp` and the bootstrap payload
- File:line: `src/common/changepw.rs:1-18, 54-84`; `src/common/bootstrap_message.rs:1-36`; `test-vectors/README.md:43-52`; `src/common/tests/test_vectors_tests.rs` (8 vectors, neither included).
- Issue: the changepw module doc demands `auth_message_cp` be "byte-exactly reproduced by both sides", and `build_bootstrap_input` is the payload the client pins the server identity against — yet both are covered only by Rust-to-Rust round-trips (`integration_changepw.rs`, `integration_bootstrap.rs`). The vector suite whose stated purpose is protecting "cross-language interop" omits exactly these two composite constructions, unlike `auth_message`, `identity_input`, and the rotation payloads.
- Failure scenario: a TS/browser client implements changePassword or bootstrap from the prose layout; a length-prefix or field-order slip fails only at runtime against the real server, with no pinned bytes to diff against.
- Fix: add `auth_message_cp_default.{json,toml}` and `bootstrap_input_default.{json,toml}` pairs plus assertions in `test_vectors_tests.rs`, per the README's own "Adding new vectors" recipe.

### 5.8 — low — `validate_client_kdf_safe` returns `Result<(), String>`; the sole caller discards the diagnostic *(also flagged [low] by error lens 6.10 and as a correctness-lens nit)*
- File:line: `src/common/kdf_params.rs:77-91` (verified: `std::result::Result<(), String>`); `src/client/handshake.rs:210-212`.
- Issue: a public library API returns a bare-`String` error, against the crate's own convention (thiserror `Error` enum everywhere else, CLAUDE.md error-handling rules), and its carefully-written downgrade-attack message is thrown away at the only call site (`if let Err(_msg)` → bare `Error::KdfParamsRejected`). Tests string-match on the message text.
- Failure scenario: an operator debugging why a handshake rejects a server's KDF params gets no distinguishable reason (memory cap vs time cap); embedders must substring-match a `String`.
- Fix: return `Result<(), Error>` with a `KdfLimit { field, limit }`-style variant (or reuse `Error::KdfParamsRejected`), and log/attach the reason at the call site.

### 5.9 — low — `Session::session_id` is zero-initialized and stamped externally; stale doc leaves a redundant, unchecked `session_id` parameter
- File:line: `src/server/session.rs:126-138, 232-259` (zero init), `session.rs:435-451` (stamped at `SessionStore::insert`); `src/server/changepw.rs:113-127` (stale doc), `src/server/changepw.rs:1-7` (names nonexistent `verify_and_apply_change_password`; related stale-doc defect in the same file: 7.6).
- Issue: `verify_change_password_request_with_sid`'s doc says the explicit `session_id` parameter exists "because `Session` does not carry its own id" — it does now (public field, stamped by the store). The API takes two independent ids that are never cross-checked, and `Session::new` hands out a session whose `session_id` is all zeros (and whose `hmac_key()` is therefore derived from zeros) unless the caller knows to route through `SessionStore::insert`.
- Failure scenario: an embedder constructs a `Session` directly (or passes the wrong sid alongside the session): destructive-op confirmation tags and `auth_message_cp` bind to a zeroed/mismatched session id, with no error.
- Fix: take the sid in `Session::new` (drop external stamping), make `verify_change_password_request_with_sid` read `session.session_id` (or `debug_assert_eq!` the two), and refresh the stale module/function doc names.

### 5.10 — nit — `RateLimiter` trait doc says "sliding-window rate limiter"; the implementation (and module doc) is a token bucket
- File:line: `src/server/rate_limit.rs:89`.

### 5.11 — nit — `push_envelope_tests.rs` round-trip iterates 4 of 5 `PushKind` variants; `Ready` is never exercised
- File:line: `src/common/tests/push_envelope_tests.rs:4-10`.

### 5.12 — nit — `RequestEnvelopeRef.session_id` hard-codes `&'a [u8; 32]` while the rest of the crate uses `limits::SESSION_ID_BYTES`
- File:line: `src/common/envelope.rs:95`.

### 5.13 — nit — `kdf_upgrade_required: Option<bool>` models a boolean in three states; plain `bool` (or an enum) would remove the `Some(false)` ambiguity
- File:line: `src/client/handshake.rs:73` / `src/server/handshake.rs:103`.

### 5.14 — nit — *(dedup: same defect as 1.12)* `with_capacity` 142 vs 144. ### 5.15 — nit — *(dedup: same defect as 7.5)* stale `test-vectors/auth_v1/` doc path.

---

## 6. error-handling-lifecycle

The crate is largely on-convention here (single thiserror `Error` enum with wire-privacy collapse, `Result` propagation throughout, RAII `Argon2Permit`, logged/propagated snapshot-sink errors in lockout/rate-limit); the gaps concentrate in the error paths themselves.

### 6.1 — high — `FjallConsumedCounters::try_advance` conflates persistence failure with replay, mutates state before durability, and logs nothing
- File:line: `crates/shamir-connect/src/server/durable_counters.rs:115-151` (get err → `false` at 128; insert at 140; persist check at 147-149); trait contract at `src/server/resume.rs:40-55`.
- Issue: `ConsumedCounterStore::try_advance` returns `bool`, so the only durable implementation must fold three very different outcomes — read error, insert error, fsync error — into the same `false` that means "replay/stale" upstream (`process_resume` maps it to generic `Error::AuthFailed`). Worse, `keyspace.insert` (line 140) lands the new counter in fjall's in-memory journal *before* `persist(PersistMode::SyncAll)` is attempted (line 147); on persist failure the method returns `false` but the advanced counter is already visible to every later read. There is no `log::warn!` anywhere in `try_advance` (contrast `gc` at lines 178/183) despite `Cargo.toml:46-48` adding the `log` dependency precisely because persistence failures "must surface to operators".
- Failure scenario: transient I/O failure at exactly the fsync step: the client's resume fails with `authentication_failed`, the journalled counter is nonetheless durable-visible, and the client's retry with the same ticket now hits `new_counter > c == false` — the ticket family is permanently bricked (recovery only via full SCRAM re-auth). Meanwhile operators see silent auth-failure spikes with no diagnostic; a read-error outage is indistinguishable from a replay attack.
- Fix: log every error branch at `warn`/`error` (matching `gc`). Change the trait to return an enum or `Result` (e.g. `Accepted / Replayed / StorageError`) so `process_resume` can fail with `ServerBusy`-style semantics instead of `AuthFailed` on storage trouble. Where the `bool` shape must stay, best-effort roll back the journalled insert on persist failure and log loudly. Add fault-injection tests (wrapper around the db handle) pinning post-failure behavior — `durable_counters_tests.rs` covers only happy paths and restart durability.

### 6.2 — medium — Client password slices are not zeroized on the error path, contradicting their doc contracts *(also flagged [low] by correctness 1.7 and security 3.3)*
- File:line: `crates/shamir-connect/src/client/handshake.rs:231-232`; `crates/shamir-connect/src/client/bootstrap.rs:96-97`; `crates/shamir-connect/src/client/changepw.rs:60-61 and 71-72`.
- Issue: all three flows call `DerivedKeys::derive(password, ...)?` and only then `password.zeroize()`. The `?` returns early on derivation failure, skipping the zeroize. `ClientHandshake::process_challenge`'s doc says "`password` is consumed and zeroized on return" — unconditional on its face; `build_request` in bootstrap/changepw similarly promise zeroize "after use". The error path is reachable: `validate_client_limits` (`common/kdf_params.rs:34-43`) checks only *upper* bounds, so a misbehaving/compromised server can send degenerate params (e.g. `memory_kb < 8*parallelism`) that pass validation and then fail inside `argon2id` → `Params::new` / `hash_password_into` (`common/crypto.rs:158-165`). Every attacker-influenceable rejection — over-cap KDF params, all-zero server nonce, password-policy failure — leaves the raw password resident in caller memory.
- Failure scenario: a hostile server deliberately replies with `kdf_params_rejected`-triggering (or degenerate) parameters; the client's password lingers in freed-but-unwiped heap that a later core dump or heap-grooming attacker can recover, undermining the crate's otherwise strict zeroization discipline (Zeroizing keys, redacted Debug impls).
- Fix: zeroize on scope exit regardless of result — a small drop-guard wrapping each `&mut [u8]` password slice — or explicit zeroize before each early `return`/`?`. Add an error-path test using degenerate `KdfParams` exercising the derive-failure branch (currently zero coverage).

### 6.3 — medium — Signed-subtraction underflow panic on a backwards clock step in `changePassword` TTL check *(also flagged [low] by correctness 1.8)*
- File:line: `crates/shamir-connect/src/server/changepw.rs:141`.
- Issue: `if now_ns - pending.issued_at_ns > CHANGEPW_CHALLENGE_TTL_NS` subtracts two independently-sampled wall-clock values (`UnixNanos` is `SystemTime`-based, `common/time.rs:18-23`, documented as "NTP-disciplined"). If `now_ns` is earlier than `issued_at_ns` — an NTP step-back between challenge issue and verify, or any caller passing a stale/mock clock — the `u64` subtraction underflows: panic in debug builds, huge wrapped value in release (spurious `AuthFailed`). Everywhere else in the crate the same pattern uses `saturating_sub` (rate limiters, lockout). Coverage gap: no test exercises TTL−1 (accept), TTL+1 (reject), or a backwards clock.
- Failure scenario: server clock steps backwards ~1 s after a user requests a change-password challenge; the user's next `changePassword` submit panics the request task in a debug build (or is silently rejected in release), with the challenge already consumed by the atomic `swap(None)` at line 139.
- Fix: `match now_ns.checked_sub(pending.issued_at_ns) { Some(elapsed) if elapsed <= TTL => …, _ => Err(AuthFailed) }` (explicitly rejecting future-dated challenges), or `saturating_sub`; extend `tests/integration_changepw.rs::rejects_after_ttl_expiration` with a clock-regression case and the TTL±1 boundary tests.

### 6.4 — medium — *(primary: same defect as 7.1)* `dispatch_request` lacks the rate gate; the `rate_limited` error branch is untested
- Full write-up at 7.1. Error-lens contribution: no dispatch-level test drives a drained bucket to the `rate_limited` envelope on either entry point (existing coverage is only at the `Session::check_post_auth_rate_limit` unit level) — fold into 7.1's fix.

### 6.5 — medium — `AuditAppender` persistence failures are unreportable by design; audit chain advances silently ahead of durable storage
- File:line: `crates/shamir-connect/src/server/audit_chain.rs:341-348` (trait methods return `()`), `406-434` (`AuditChainWriter::append` calls the appender unconditionally).
- Issue: `AuditAppender::append_entry`/`checkpoint` have no error channel, and `AuditChainWriter` updates the in-memory chain (`seq`, `prev_hmac`) *before/independently* of the appender result. A failing durable appender (disk full, fsync error) loses audit events with no signal, and the truncation-defence checkpoint can then describe chain state that never reached storage — or not be written at all, equally silently. The crate's own `Cargo.toml:46-48` rationale says such failures "must surface to operators"; lockout/rate-limit snapshot sinks got `Result` + `log`, the audit path did not.
- Failure scenario: disk-full during an incident: audit events are dropped with no `log` line, metric, or error; at restart, chain verification either flags a puzzling truncation (cf. 1.3) or the gap is never noticed.
- Fix: give `append_entry`/`checkpoint` a `Result` return (or at minimum have `AuditChainWriter` log on failure via the `log` crate), and add a failing-appender test asserting the failure is observable. (Related nit: `AuditError` itself — 6.13.)

### 6.6 — low — OS RNG failures panic instead of returning `Result` *(also flagged as a correctness-lens nit, which adds `common/time.rs:18-23` pre-epoch-clock `.expect`)*
- File:line: `crates/shamir-connect/src/common/crypto.rs:66-70` (`random_bytes`), `215-219` (`Ed25519Keypair::generate`).
- Issue: `.expect("OS RNG failure")` converts an environment failure (not a programmer-invariant violation) into a panic, against the house rule. These functions sit on the nonce/session-id/ticket-family generation paths of every handshake and resume.
- Failure scenario: on a hardened system where `getrandom` can fail (seccomp filter, entropy edge, container misconfiguration), every connection task panics rather than surfacing a `ServerBusy`-class error. Practically near-unreachable on supported platforms, hence low.
- Fix: return `Result` variants (e.g. `try_random_bytes`), or keep the panic but annotate it as a deliberate unrecoverable-system-state invariant so it is visibly excepted from the house rule. Same treatment for the `time.rs` pre-epoch expect.

### 6.7 — low — `FjallConsumedCounters::open` leaks the third-party `fjall::Error` into the library API
- File:line: `crates/shamir-connect/src/server/durable_counters.rs:70`.
- Issue: returns `Result<Self, fjall::Error>`, coupling callers to an optional-dependency's error type (`durable-fjall` feature) — the only public API in the crate whose error type is neither crate `Error` nor a local thiserror type.
- Failure scenario: callers must match on `fjall::Error` (and add `fjall` to their deps to name it) to classify startup failures; a future storage-backend swap becomes a breaking change.
- Fix: wrap in a small `#[derive(thiserror::Error)]` enum (a `#[from] fjall::Error` variant is exactly the "where natural" case CLAUDE.md calls for).

### 6.8 — low — `unpack_value` panics on a malformed persisted counter value; only `gc` guards the length
- File:line: `crates/shamir-connect/src/server/durable_counters.rs:94-100` (unguarded slicing); unguarded call sites at 110 (`peek`) and 130 (`try_advance`); guard exists only in `gc` at 163.
- Issue: `unpack_value` indexes `v[8..16]` without a length check. Any truncated/corrupt 16-byte value on disk (external corruption, partial legacy write) makes `try_advance` — an authentication-path function — panic via `copy_from_slice` instead of failing closed.
- Failure scenario: one corrupt key in the counters keyspace turns every resume attempt for that (user, family) into a task panic rather than a rejection.
- Fix: return `Option<(u64, u64)>`/`Result` from `unpack_value` on `v.len() != 16` and treat as "no prior" plus a `log::warn!`, mirroring `gc`'s defensive skip.

### 6.9 — low — `process_resume` discards the validated transport enum and re-derives it with a silent fail-open default
- File:line: `crates/shamir-connect/src/server/resume.rs:276-277` (validated tuple discarded: `_transport_at_auth`), `388-389` (`TransportKind::from_u8(...).unwrap_or(TransportKind::Tcp)`).
- Issue: `validate_ticket_enums` already fails-closed on an unknown `transport_kind_at_auth`, but its transport half is thrown away and the raw byte is re-parsed at line 389 with `.unwrap_or(Tcp)`. Today the fallback is dead; if validation is ever reordered/loosened, an unknown enum silently becomes `Tcp` — a fail-open default on a security-relevant field, in a function whose documented posture is "any failure → `Error::AuthFailed`".
- Failure scenario: a future edit drops or moves the `validate_ticket_enums` call; tickets with garbage transport bytes resume successfully as `Tcp` sessions with no error anywhere.
- Fix: use the `(transport, binding)` tuple returned by `validate_ticket_enums` and delete the `unwrap_or` re-parse, making the fail-closed guarantee structural.

### 6.10 — low — *(dedup: same defect as 5.8)* stringly-typed `validate_client_kdf_safe` error.

### 6.11 — low — `Error::to_wire` — the wire-privacy collapse — is dead code workspace-wide and untested
- File:line: `crates/shamir-connect/src/common/error.rs:86-103` (no callers anywhere in the workspace).
- Issue: the helper encoding the spec §14.4 rule ("any internal cause collapses to generic `AuthFailed` on the wire") has zero call sites; the discipline is instead maintained ad hoc by `map_err(|_| Error::AuthFailed)` chains (e.g. all of `process_resume`) while `dispatch_request*` propagate internal `Error::InvalidInput("...")` detail strings to the transport layer, which must remember to collapse them itself (cf. 5.4). Nothing tests the collapse set (which variants survive vs. become `AuthFailed`).
- Failure scenario: a future path forwards an internal error (with its distinguishing message/variant) straight into an `ErrorEnvelope`; no helper, type, or test stands in the way.
- Fix: either route the transport-boundary error mapping through `to_wire` (and unit-test the preserved set: `RateLimited`/`ServerBusy`/`UnsupportedVersion`/`BootstrapFailed`) or delete it and document the per-call-site `map_err` convention explicitly. Decide together with 5.4.

### 6.12 — low — Consolidated error-path test gaps
- File:line: various.
- Issue: beyond the gaps noted per finding (6.1 durable-counter failures, 6.2 zeroize-on-error, 6.3 clock regression, 6.4 dispatch `rate_limited` branch, 6.5 failing audit appender): (a) no test exercises `LockoutSnapshotSink::save`/`RateLimitSnapshotSink::save` *failure* propagation through `persist_snapshot` (only success and no-sink paths in `lockout_tests.rs:362-383`, `rate_limit_tests.rs:207-238`); (b) no client-side test drives the Argon2-failure branch of `DerivedKeys::derive` via degenerate KDF params.
- Failure scenario: regressions in exactly these branches (the ones that touch storage, secrets, or fail-open defaults) would land green.
- Fix: fault-injection sinks (failing `save`) and degenerate-param cases in the respective test files; both need no new production code.

### 6.13 — nit — `AuditError` is hand-rolled instead of thiserror, with `Display` = `Debug`
- File:line: `crates/shamir-connect/src/server/audit_chain.rs:297-334`.
- Issue: manual `Display` that just forwards `{:?}` plus an empty `std::error::Error` impl, where every comparable error in the crate uses `thiserror` with human-readable messages.
- Failure scenario: operator-facing output renders `SequenceGap { at: 3, expected: 4, found: 7 }` instead of a sentence; field additions silently change log format.
- Fix: `#[derive(Debug, thiserror::Error)]` with per-variant `#[error("...")]` strings.

---

## 7. style-claude-md

Structural pillars hold: all three `mod.rs` manifests are re-export-only, zero inline `#[cfg(test)]` blocks in `src/`, mandated `tests/` layout, one-file-one-export respected (multi-type files are cohesive families), benches on the workspace-mandated `bench_scale_tool::Harness`. The debt is comment/doc discipline — including the crate's worst finding.

### 7.1 — high — **`dispatch_request_view` doc claims "functionally identical" but only it enforces the post-auth rate gate** *(primary of the crate's most cross-flagged defect: also [med] correctness 1.5, [med] security 3.1, [med] api 5.2, [med] error 6.4, plus a sub-note in concurrency 2.1)*
- File:line: `crates/shamir-connect/src/server/dispatch.rs:112-117` (doc) vs `dispatch.rs:150-160` (gate; verified: `check_post_auth_rate_limit` occurs exactly once in the file, at `:153`); counterpart `dispatch.rs:69-110` (no gate); `dispatch_request` `pub use`d at `src/server/mod.rs:28`.
- Issue: the doc on `dispatch_request_view` states it is "**Optim #4** zero-copy variant of `dispatch_request` … Functionally identical: same §7.5 validity check, same handler dispatch, same outcome shape", and its rate-limit comment claims a "single choke point covering every transport that routes through this function". That is false: only the view variant runs the task-#608 `check_post_auth_rate_limit` choke point; the owning `dispatch_request` has no rate-limit block. The doc actively misleads about a security control on a public API pair.
- Failure scenario: production currently routes through the gated variant (`shamir-server/src/connection/request_loop.rs:340`), but `dispatch_request` is the variant used by tests/benches/`shamir-transport-tcp/tests/echo_e2e.rs` and is exported as a peer API. An integrator who reads "functionally identical" and picks the owning variant silently loses all per-session request-rate limiting; authenticated sessions can hammer handlers unthrottled, bounded only by transport-level limits.
- Fix: (a) add the same `check_post_auth_rate_limit` block to `dispatch_request` (dedupe the common prefix into one shared helper), or (b) deprecate/remove the owning variant from the public re-exports and mark it test/bench-only with a doc warning. Either way, correct the doc on both functions, and add the dispatch-level test asserting a drained bucket yields the `rate_limited` envelope on the surviving path(s) (closes 6.4's gap). Option (a) is preferable; the asymmetry has no principled reason.

### 7.2 — medium — *(dedup: same defect as 5.3)* `encode_details_canonical` public stub whose doc promises encoding it never performs.

### 7.3 — medium — Lock sites on runtime structs missing the inline contention-model comments CLAUDE.md mandates *(overlaps 2.2, 2.3, 2.5 — comment-discipline facet; the restructure-vs-comment decisions belong to those findings)*
- File:line: `crates/shamir-connect/src/server/session.rs:416` (`cap_lock: Mutex<()>` — no comment at all); `crates/shamir-connect/src/server/audit_chain.rs:131` (`inner: Mutex<ChainInner>` — doc describes contents, never the contention model); `crates/shamir-connect/src/server/argon2_semaphore.rs:36` (`notify: (std::sync::Mutex<()>, Condvar)`); `crates/shamir-connect/src/server/admin.rs:298` (`next_id: parking_lot::Mutex<u128>`); `crates/shamir-connect/src/server/bootstrap.rs:49` (`inner: Mutex<BootstrapInner>`).
- Issue: CLAUDE.md: "Every hot-path use must be justified inline with a comment that names the contention model", and for `std::sync::Mutex` the F-9/#1076 revision states a new instance "must fit one of these [three sanctioned] categories (with its own inline comment naming the model) … it may NOT cite 'precedent' … Each carries its own inline comment — **that is the enforcement mechanism**." None of these five sites carries such a comment. Strongest instance: `cap_lock` is taken on every per-user-capped insert (the auth path) and its critical section includes a full `DashMap` iteration over ALL users' sessions — precisely the shape of use the rule exists to police — with zero justification (structural fix: 2.2). Contrast: `durable_counters.rs:16-20` carries exactly the required comment — the convention is known in this crate but inconsistently applied.
- Failure scenario: the documented enforcement mechanism cannot distinguish sanctioned sites from drift — the exact recurrence CLAUDE.md's own F-9 revision warns about. Future concurrency audits must re-derive each contention model from scratch, and an unwary editor can grow `cap_lock`'s critical section (it already scans the whole store) with no in-code signal.
- Fix: add the mandated one-line contention-model comment at each site (e.g. `cap_lock`: "per-auth admin op, critical section must stay O(sessions-of-user), contention nil" — moot if 2.2 removes the lock); for `argon2_semaphore` either document why a Condvar wait-queue is acceptable under the pillar rules (atomic fast path, mutex only on the saturated wait path) or migrate to `tokio::sync::Semaphore` (see 2.5).

### 7.4 — low — Dead suppressors keeping unused imports alive in `server/handshake.rs` *(also flagged as a correctness-lens nit)*
- File:line: `crates/shamir-connect/src/server/handshake.rs:232` (`let _ = constant_time_eq;`), `handshake.rs:388-390` (`#[allow(dead_code)] fn _doc_link_targets`), keeping alive the unused imports at `handshake.rs:17` and `handshake.rs:27-28`.
- Issue: `constant_time_eq` is imported but never actually used; line 232 burns the import with a no-op statement placed directly under the "Branch ONLY on the accept/reject decision" comment, making dead code look like part of the constant-time discipline. `_doc_link_targets` exists solely so `ResumeConfig`/`ServerIdentityState` imports don't warn — but nothing references them.
- Failure scenario: readers of `verify_proof` may assume the `let _ =` line performs a timing-neutralization (it does nothing); future edits keep propagating the unused imports.
- Fix: remove `constant_time_eq`, `ResumeConfig`, and `ServerIdentityState` from the import block and delete both suppressors; use fully-qualified intra-doc links if wanted.

### 7.5 — low — Stale doc references to artifacts that don't exist under the given names *(also flagged as nits by correctness 1.16 and api 5.15)*
- File:line: `crates/shamir-connect/src/common/auth_message.rs:6`; `crates/shamir-connect/src/common/envelope.rs:89-90`.
- Issue: `auth_message.rs` says test vectors live in `crates/shamir-connect/test-vectors/auth_v1/` — a directory that never exists (vectors are `test-vectors/*.{json,toml}`; `test-vectors/README.md:19-23` itself records that the `auth_v1` name was spec prose that "never existed" as a file layout). `envelope.rs` cites the integration test `request_envelope_ref_wire_compat_with_owning`; the actual test is `request_envelope_ref_wire_compat_with_owning_and_view` (`tests/integration_session.rs:333`).
- Failure scenario: a reader following the `auth_v1/` path (e.g. a TS-SDK implementer verifying byte-exactness, which this file's doc says is mandatory) finds nothing and may conclude the vectors are missing.
- Fix: point `auth_message.rs` at `test-vectors/auth_message_default.{json,toml}` and update the test name in `envelope.rs`.

### 7.6 — low — `finalize_change_password` doc header is a stale merge from the verify function
- File:line: `crates/shamir-connect/src/server/changepw.rs:92-104`.
- Issue: the function's doc opens with "Step 2: server verifies the request and (on success) returns the new material to persist. The pending challenge is cleared regardless of outcome (single-use), and on success ALL sessions of the user must be killed by the caller…" — that text describes `verify_change_password_request_with_sid` (properly documented at lines 113-122; see also 5.9 for the same file's stale names), before the actual one-line description ("Helper: kill all sessions for a user…"). The function does none of the verify/challenge work.
- Failure scenario: a reader wiring the changePassword flow may call `finalize_change_password` believing it consumes the pending challenge and validates the proof, and skip `verify_change_password_request_with_sid` — leaving the single-use challenge unconsumed (re-playable within TTL) and the proof never checked.
- Fix: delete the stale "Step 2 …" paragraphs; keep the "Helper: kill all sessions…" description and the §12.5.3 citation.

### 7.7 — low — Mid-function `use` statements violating the imports-at-top rule
- File:line: `crates/shamir-connect/src/server/admin.rs:324` (`use dashmap::mapref::entry::Entry;` inside `InMemoryUserDirectory::insert` — the only non-test violation in `src/`); `crates/shamir-connect/src/server/tests/post_auth_rate_limit_tests.rs:98-99` (`use std::sync::{Arc, Barrier}; use std::thread;` inside the test fn — nit).
- Issue: none of CLAUDE.md's three documented exceptions applies (no collision-annotated trait import, not cfg-gated, not `use super::*` in a test module).
- Fix: hoist `Entry` to `admin.rs`'s header; hoist `Arc`/`Barrier`/`thread` to the test file's header block at lines 10-12.

---

## Finding counts

Raw lens-tagged counts (each explicitly severity-tagged item counted once per lens file; nit bundles counted per tagged bullet; style 7.7 counted once):

| Severity | correctness | concurrency | security | performance | api/wire | error/lifecycle | style | **lens-tagged total** |
|---|---|---|---|---|---|---|---|---|
| critical | 0 | 0 | 0 | 0 | 0 | 0 | 0 | **0** |
| high | 1 | 2 | 0 | 2 | 1 | 1 | 1 | **8** |
| medium | 4 | 2 | 1 | 1 | 4 | 4 | 2 | **18** |
| low | 6 | 3 | 4 | 1 | 4 | 7 | 4 | **29** |
| nit | 8 | 1 | 3 | 1 | 6 | 1 | 0 | **20** |
| **total** | 19 | 8 | 8 | 5 | 15 | 13 | 7 | **75** |

*(The workspace SUMMARY row shows 76/19 med — a one-item counting-convention divergence vs. this file, most likely the dual "low (nit for the test file)" tag on style 7.7 or a bundle item counted differently. Immaterial post-dedup.)*

Deduplicated distinct-defect census (same root-cause defect flagged by multiple lenses counted once, under its primary lens):

| Severity | Distinct | Defects (primary lens.number; group companions in parens) |
|---|---|---|
| critical | 0 | — |
| high | 7 | 1.1 (TOFU ordering) · 2.1 (fetch_max refill race — headline) · 2.2 (+4.1; style-7.3 overlap) (cap_lock O(N)-scan) · 4.2 (AuditChain unbounded Vec) · 5.1 (+1.4) (client feature build) · 6.1 (durable-counter conflation) · 7.1 (+1.5, 3.1, 5.2, 6.4; sub-note in 2.1) (ungated dispatch twin) |
| medium | 12 | 1.2 (sibling watermark regress) · 1.3 (checkpoint false positive) · 2.3 (+4.4; style-7.3 overlap) (audit-lock CS) · 2.4 (fsync doc drift) · 4.3 (resume uncapped insert) · 5.3 (+1.9, 3.7, 7.2) (encode_details stub) · 5.4 (handler `Err(String)` on wire) · 5.5 (`PushEnvelope.data` not `serde_bytes`) · 6.2 (+1.7, 3.3) (password zeroize on error) · 6.3 (+1.8) (TTL underflow) · 6.5 (appender failures unreportable) · 7.3 (missing contention-model comments) |
| low | 21 | 1.10 (vacuous nonce assertions) · 1.11 (`with_rate(0)`) · 2.5 (blocking semaphore surface) · 2.6 (lockout hasher) · 2.7 (+1.6, 3.4) (rotate/finalize race) · 3.2 (server secrets unzeroized) · 3.5 (KDF enumeration, accepted) · 5.6 (ticket version literal) · 5.7 (missing cp/bootstrap vectors) · 5.8 (+correctness-nit, 6.10) (stringly KDF error) · 5.9 (zero-init session_id/stale doc) · 6.6 (+correctness-nit) (RNG/clock panics) · 6.7 (`fjall::Error` leak) · 6.8 (`unpack_value` panic) · 6.9 (resume fail-open transport) · 6.11 (`to_wire` dead) · 6.12 (error-path test gaps) · 7.4 (+correctness-nit) (dead suppressors) · 7.5 (+2 nits) (stale doc refs) · 7.6 (stale doc header) · 7.7 (mid-function `use`) |
| nit | 13 | 1.12 (+api-nit) (capacity const) · 1.13 (unused dep) · 1.14 (u16 truncation) · 1.15 (threshold asymmetry) · 2.8 (Debug label) · 3.6 (zero nonce_cp accepted) · 3.8 (canonical length truncation) · 4.5 (owning-variant clock syscall) · 5.10 (sliding-window doc) · 5.11 (`Ready` unexercised) · 5.12 (hard-coded 32) · 5.13 (`Option<bool>`) · 6.13 (hand-rolled `AuditError`) |
| **total** | **53** | |

Dedup accounting: **75 lens-tagged findings → 53 distinct defects** (22 folded instances across 13 cross-lens groups: dispatch gate ×5, cap_lock ×2, audit-lock CS ×2, rotate/finalize ×3, password zeroize ×3, encode_details ×4, client feature ×2, TTL underflow ×2, KDF stringly ×3, RNG panics ×2, capacity const ×2, stale vector path ×3, dead suppressors ×2). Verdict consistent with the workspace scorecard (#7, "needs focused remediation"): no criticals, but seven highs that all sit on the auth/security spine.

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Fix the TOFU ordering.** Run `verify_identity` before the pin branch (or defer the callback to just before `Ok(HandshakeSuccess)`); red test: TOFU + tampered `identity_sig` → `Err` and no callback invocation. Closes **1.1** — removes the persistent-MITM pinning path.
2. **Fix the post-auth refill race at both sites.** Pack `(watermark, tokens)` into one `AtomicU64` with a `fetch_update` CAS loop (or load the watermark inside the `fetch_update` closure) in `PostAuthBucket`, and apply `max()`-guarded watermark advance in `InMemoryRateLimiter::check`; correct the "bounded by wall-clock span" doc. Add the missing tests: a `Barrier` race with `now > watermark`, and the out-of-order-`now_ns` test ported to `rate_limit_tests.rs`. Closes **2.1** (headline) and its sibling **1.2**.
3. **Make the dispatch entry points honest.** Add the `check_post_auth_rate_limit` gate to `dispatch_request` via a shared helper (or remove it from `server/mod.rs` re-exports), fix the "functionally identical" docs, and add the dispatch-level `rate_limited`-envelope test. Closes **7.1**, **1.5**, **3.1**, **5.2**, **6.4** (and retires most of **4.5**).
4. **Restructure the per-user session cap.** Per-user secondary index (Fx-hashed `DashMap`/`scc::HashMap` of sid sets, updated in insert/remove/kick/GC) replacing the O(all-sessions) scan under `cap_lock`; then route `process_resume` through the capped insert and plumb the evicted sid out. Closes **2.2**, **4.1**, **4.3**.
5. **Harden the durable replay counter.** Log every `try_advance` error branch; return `Accepted/Replayed/StorageError` (or roll back the journalled insert on persist failure); guard `unpack_value`; add fault-injection tests. Closes **6.1** and **6.8** — stops fsync errors from silently bricking ticket families.
6. **Bound the audit chain's memory.** Default chain keeps only `(next_seq, prev_hmac)` + streams to the appender (or a bounded ring, `Arc<AuditEntry>` if retention is kept). Closes **4.2** — removes the OOM-by-design.
7. **Fix the feature matrix.** `client = ["server"]` + README correction (or move the shared wire views to `common/`); verify with `--no-default-features --features client`. Closes **5.1**, **1.4**.

**P1 — soon**

8. **Audit-chain hygiene.** `Result` (or at minimum loud `log`) from `AuditAppender::append_entry`/`checkpoint` with a failing-appender test (**6.5**); shrink the `append` critical section (reserve/publish, `Arc<AuditEntry>`) (**2.3**, **4.4**); delete or properly implement the `encode_details_canonical` stub with a canonical-bytes test (**5.3**, **1.9**, **3.7**, **7.2**); add the mandated contention-model comments at all five sites (**7.3**) and fix the `write_lock`/fsync doc (**2.4**).
9. **Zeroize passwords on all exits.** Drop-guards over the password slices in handshake/bootstrap/changepw + degenerate-`KdfParams` error-path tests. Closes **6.2**, **1.7**, **3.3**.
10. **Clock-safety in changePassword.** `checked_sub`-based TTL check (explicitly reject future-dated) + TTL±1 and clock-regression tests. Closes **6.3**, **1.8**.
11. **Fix `verify_against_checkpoint` semantics** (truncation ⇔ checkpoint ahead by >1; HMAC check only at `last_seq + 1`) + stale-checkpoint red test. Closes **1.3**.
12. **Close the wire-leak and dead-helper pair.** Type the handler error as crate `Error` (or pin the §14 vocabulary with a test) (**5.4**) and either adopt `to_wire` with a collapse-set test or delete it and document the `map_err` convention (**6.11**).
13. **`serde_bytes` for `PushEnvelope.data`** + byte-exact frame pin. Closes **5.5**.
14. **Make identity rotation atomic.** `ArcSwap` CAS loop with in-loop precondition re-check; `try_finalize` refreshes the mirror; interleaving test. Closes **2.7**, **1.6**, **3.4**.
15. **`RateLimiter` input validation.** Reject/clamp `rate_per_sec = 0`. Closes **1.11**.

**P2 — backlog**

16. **Zeroize the long-lived server secrets** (`Zeroizing<[u8; 32]>` for `server_secret`/`lockout_secret`/ticket keys). Closes **3.2**.
17. **Doc-and-pin the KDF enumeration trade-off** (spec §13.5) or pad Argon2id to the worst case. Closes **3.5**.
18. **Public-API cleanups:** ticket wire version constant + encrypt-side validation (**5.6**); `auth_message_cp`/bootstrap byte-exact vectors (**5.7**); `validate_client_kdf_safe` → typed error (**5.8**); `Session::new` takes the sid + changepw doc refresh (**5.9**, with **7.6**); wrap `fjall::Error` (**6.7**); `Result` for RNG/time `.expect`s (**6.6**); convert `AuditError` to thiserror (**6.13**).
19. **Concurrency-p behavioral fixes:** rename/guard/delete `Argon2Semaphore::acquire*` (**2.5**); Fx hasher for `lockout.rs` maps (**2.6**); delete the `unwrap_or(Tcp)` re-parse in `process_resume` (**6.9**).
20. **Test-gap sweep:** vacuous `matches!` assertions → `assert!` (**1.10**); fault-injection snapshot-sink tests + Argon2-failure branch coverage (**6.12**); exercise `PushKind::Ready` (**5.11**).
21. **Style/doc sweep (one docs-only pass):** stale `auth_v1`/envelope-test references (**7.5**); dead suppressors + unused imports in `server/handshake.rs` (**7.4**); mid-function `use` hoists (**7.7**); capacity const fix (**1.12**); unused `unicode-normalization` dep (**1.13**); `u16` ticket-length `debug_assert!` (**1.14**); rotation threshold asymmetry note (**1.15**); `Debug` label (**2.8**); zero-`nonce_cp` fail-fast (**3.6**); canonical-bytes length rejection (**3.8**); owning-variant doc/syscall note (**4.5**); "sliding-window" wording (**5.10**); `SESSION_ID_BYTES` constant (**5.12**); `Option<bool>` → typed (**5.13**); delete-or-implement `encode_details_canonical` is already covered by P1 item 8.

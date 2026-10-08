<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-connect — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The headline multiplicative fetch_max race is refuted. The subnet regression, global capped-insert scan, identity-state update races, and documentation issues remain.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 7 | 0 | 0 | 1 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Concurrent callers share the pre-fetch_max refill watermark and multiply refill by racer count

Status: `refuted`. Current risk: —.

fetch_max is an atomic read-modify-write, not an independently delayed load/store. For identical later timestamps, one caller receives the old watermark; subsequent callers receive the advanced value and compute zero elapsed. Successful token CAS updates apply each caller's refill once. The alleged N-fold shared-span credit cannot follow this code.

Evidence: [crates/shamir-connect/src/server/session.rs:353](../../../../../crates/shamir-connect/src/server/session.rs#L353); [crates/shamir-connect/src/server/session.rs:357](../../../../../crates/shamir-connect/src/server/session.rs#L357); [crates/shamir-connect/src/server/session.rs:370](../../../../../crates/shamir-connect/src/server/session.rs#L370).

<a id="review-2"></a>

### Claim 2 — SessionStore::cap_lock: unjustified parking_lot::Mutex held across an O(all-sessions) scan

Status: `confirmed-open`. Current risk: `high`.

Every capped insertion takes one global lock, traverses all sessions, and sorts the user's matches. Production SCRAM completion calls it. Complexity and serialization are proven; the numerical latency scenario is unmeasured.

Evidence: [crates/shamir-connect/src/server/session.rs:416](../../../../../crates/shamir-connect/src/server/session.rs#L416); [crates/shamir-connect/src/server/session.rs:470](../../../../../crates/shamir-connect/src/server/session.rs#L470); [crates/shamir-connect/src/server/session.rs:476](../../../../../crates/shamir-connect/src/server/session.rs#L476); [crates/shamir-server/src/connection/handshake.rs:580](../../../../../crates/shamir-server/src/connection/handshake.rs#L580).

Grouping/duplicate: `performance-hotpath.md#1`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — AuditChain::inner: undocumented mutex containing HMAC, allocations, and an entry clone

Status: `confirmed-open`. Current risk: `low`.

The critical section still includes materialization, canonical bytes, HMAC, and retention cloning. Chain ordering genuinely requires coordination. Actual contention is unmeasured; the production sink already handles blocking fsync with block_in_place on multi-thread runtimes.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:131](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L131); [crates/shamir-connect/src/server/audit_chain.rs:196](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L196); [crates/shamir-connect/src/server/audit_chain.rs:210](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L210); [crates/shamir-server/src/audit_appender.rs:674](../../../../../crates/shamir-server/src/audit_appender.rs#L674).

Grouping/duplicate: `performance-hotpath.md#4`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — FjallConsumedCounters doc claims lock is not held across fsync

Status: `confirmed-open`. Current risk: `nit`.

The guard remains in scope through persist(SyncAll), contrary to the field comment. This is documentation drift, not independently demonstrated runtime damage.

Evidence: [crates/shamir-connect/src/server/durable_counters.rs:52](../../../../../crates/shamir-connect/src/server/durable_counters.rs#L52); [crates/shamir-connect/src/server/durable_counters.rs:124](../../../../../crates/shamir-connect/src/server/durable_counters.rs#L124); [crates/shamir-connect/src/server/durable_counters.rs:147](../../../../../crates/shamir-connect/src/server/durable_counters.rs#L147).

<a id="review-5"></a>

### Claim 5 — Argon2Semaphore exposes blocking Mutex+Condvar acquisition as an async adoption trap

Status: `confirmed-open`. Current risk: `nit`.

Blocking acquisition remains public. Method and struct docs already say it blocks, but the module's async wording lacks an explicit off-runtime requirement. No current async request-path caller of these blocking methods was found.

Evidence: [crates/shamir-connect/src/server/argon2_semaphore.rs:10](../../../../../crates/shamir-connect/src/server/argon2_semaphore.rs#L10); [crates/shamir-connect/src/server/argon2_semaphore.rs:29](../../../../../crates/shamir-connect/src/server/argon2_semaphore.rs#L29); [crates/shamir-connect/src/server/argon2_semaphore.rs:84](../../../../../crates/shamir-connect/src/server/argon2_semaphore.rs#L84).

<a id="review-6"></a>

### Claim 6 — lockout.rs DashMaps use the default RandomState hasher

Status: `confirmed-open`. Current risk: `nit`.

Both maps still omit the workspace Fx hasher. This is a convention difference, not a security failure; the 2–5x slowdown is unmeasured and replacing a keyed hasher requires threat-model review.

Evidence: [crates/shamir-connect/src/server/lockout.rs:256](../../../../../crates/shamir-connect/src/server/lockout.rs#L256); [crates/shamir-connect/src/server/lockout.rs:257](../../../../../crates/shamir-connect/src/server/lockout.rs#L257); [CLAUDE.md:339](../../../../../CLAUDE.md#L339).

<a id="review-7"></a>

### Claim 7 — ServerIdentityState::rotate is a non-atomic check-then-store on ArcSwap

Status: `confirmed-open`. Current risk: `low`.

Separate load, condition, and store permit lost rotations. try_finalize can also overwrite a rotation when it acts on an expired older overlap snapshot; it is not harmless in that interleaving. The version mirror is stored separately.

Evidence: [crates/shamir-connect/src/server/rotation.rs:152](../../../../../crates/shamir-connect/src/server/rotation.rs#L152); [crates/shamir-connect/src/server/rotation.rs:169](../../../../../crates/shamir-connect/src/server/rotation.rs#L169); [crates/shamir-connect/src/server/rotation.rs:173](../../../../../crates/shamir-connect/src/server/rotation.rs#L173); [crates/shamir-connect/src/server/rotation.rs:194](../../../../../crates/shamir-connect/src/server/rotation.rs#L194).

<a id="review-8"></a>

### Claim 8 — Stale Debug label reports permissions as <RwLock>

Status: `confirmed-open`. Current risk: `nit`.

Debug still labels a plain permissions snapshot as a lock.

Evidence: [crates/shamir-connect/src/server/session.rs:111](../../../../../crates/shamir-connect/src/server/session.rs#L111); [crates/shamir-connect/src/server/session.rs:148](../../../../../crates/shamir-connect/src/server/session.rs#L148).

<a id="review-summary-guarantees"></a>

### Claim Summary.guarantees — RCU snapshots, atomic challenge consumption, no guards across await, and no scc len use

Status: `not-applicable`. Current risk: —.

These source-level non-findings hold in the inspected paths. Challenge consumption uses swap(None); dispatch releases map lookup guards before awaiting. This is not a proof that every concurrent state transition is linearizable.

Evidence: [crates/shamir-connect/src/server/changepw.rs:139](../../../../../crates/shamir-connect/src/server/changepw.rs#L139); [crates/shamir-connect/src/server/session.rs:507](../../../../../crates/shamir-connect/src/server/session.rs#L507); [crates/shamir-connect/src/server/dispatch.rs:100](../../../../../crates/shamir-connect/src/server/dispatch.rs#L100); [crates/shamir-connect/src/server/rotation.rs:23](../../../../../crates/shamir-connect/src/server/rotation.rs#L23).

## Corrections and qualified non-findings

- Remove the N-racer refill multiplication and 64x throughput claims. Relaxed ordering does not make an atomic read-modify-write non-atomic.
- The existing #1090 stale-timestamp test is weaker than its comment: replacing fetch_max with swap still rejects that last stale call. A subsequent newer call is needed to expose regression.
- A reserve/publish audit optimization must preserve chain and durable append ordering; reserving a placeholder HMAC alone is not a valid chain update.
- Current server authentication does not establish the report's claim that blocking acquisition is used in async production. Treat this as API guidance debt.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-connect -- Concurrency & lock-free invariants

## Summary

The crate is largely faithful to CLAUDE.md's five pillars: `ServerIdentityState` is RCU via `ArcSwap` with an atomic version mirror, the changepw challenge uses an `ArcSwapOption` single-submit `swap(None)` guard, the DashMaps in `session.rs`/`rate_limit.rs`/`resume.rs`/`admin.rs` are keyed with the rustc-hash `FxHasher` (byte-identical to `shamir_collections::THasher`, justified in Cargo.toml), no lock is held across any `.await`, and there is no `scc::*::len()` use. Two hot-path violations stand out: `SessionStore::cap_lock` is an unjustified `parking_lot::Mutex` on the per-auth path whose critical section is a full-store O(N) scan, and `PostAuthBucket::check_post_auth_rate_limit` — despite a doc comment claiming otherwise — does not bound total refill by wall-clock span under concurrent callers, so N racers sharing one watermark re-credit the same interval N times and defeat the per-session rate limit. Remaining findings are smaller: an undocumented audit-chain mutex with crypto+allocations inside its critical section, doc/code drift on the fjall `write_lock`, and pillar-4 drift in `lockout.rs`.

## Findings

### 1. Concurrent callers share the pre-`fetch_max` refill watermark and multiply refill by racer count — documented invariant does not hold
- **File:line:** `crates/shamir-connect/src/server/session.rs:353-358` (claim at `session.rs:207-215` and `337-352`)
- **Severity:** high
- **Issue:** `check_post_auth_rate_limit` computes `elapsed = now_ns - prev_refill_ns` from the value returned by `last_refill_at_ns.fetch_max(now_ns)`. For a *sequential* call sequence this telescopes and total refill is bounded by wall-clock span, exactly as the doc claims ("the total refill across any sequence of calls is bounded by the true wall-clock span between them"). But k *concurrent* callers that all reach `fetch_max` before any of their stores commit all observe the *same* pre-existing watermark and each independently computes `elapsed = now - watermark` — the same span is credited k times. The debit side (`micro_tokens` CAS) is exact; the refill side is not. The doc's security-invariant claim is therefore false under concurrency, and the two-atomics design cannot express "refill + watermark advance + debit" as one linearization point.
- **Failure scenario:** rate = `POST_AUTH_RATE_LIMIT_PER_SEC` = 500/s, burst capacity 500. Attacker holds one bearer token over 64 connections (`dispatch_request_view` gates per request, and the same `session_id` is shared across connections). Drain the bucket at t0. At t0+100ms fire 64 simultaneous requests: each racer reads watermark t0, credits 100ms×500/s = 50 tokens, debits 1 → all 64 admit and ~2.4k tokens of surplus remain (clamped at capacity). Repeat batches every 100ms → sustained ≈ 64 × 500 req/s from one "session", i.e. the limiter enforces `nominal / 1` only when callers serialize, which is precisely the condition it exists to not rely on. The #1090 concurrency test (`server/tests/post_auth_rate_limit_tests.rs:97-131`) races all callers at `now_ns == watermark` so `elapsed == 0` for every racer — it pins debit atomicity but is structurally blind to this refill race.
- **Suggested fix:** make refill and watermark advance part of the same atomic op: pack `(watermark, tokens)` into one `AtomicU64` and do refill+debit+advance in a single `fetch_update` CAS loop (quantize the watermark — e.g. µs or ms resolution — so both fields fit; re-derive `retry_after` from the packed state). Alternatively keep two atomics but load the watermark *inside* the `fetch_update` closure so each committed attempt prices refill off the freshest watermark (residual multi-credit shrinks to the CAS window), and correct the doc comment. Add a regression test with `now > watermark` under a `Barrier` (the existing single-instant test cannot catch this). Sub-note while here: `dispatch_request` (`server/dispatch.rs:69-110`) never calls `check_post_auth_rate_limit` — only `dispatch_request_view` does — so its "single choke point" comment holds only if every transport uses the view variant.

### 2. `SessionStore::cap_lock`: unjustified `parking_lot::Mutex` on the per-auth hot path, held across an O(all-sessions) full-store scan
- **File:line:** `crates/shamir-connect/src/server/session.rs:416` (lock), `470-493` (critical section)
- **Severity:** high
- **Issue:** CLAUDE.md is explicit: `parking_lot::*` is banned in hot paths, and every hot-path use must be justified inline with a comment naming the contention model. `cap_lock` has no such comment anywhere (struct field, `new`, or `insert_with_per_user_cap`). Worse, the critical section it guards iterates the **entire** `by_sid` DashMap to collect the inserting user's sessions (`for entry in self.by_sid.iter()`, line 476) — O(total sessions in the store), not O(sessions of that user) — under a lock that serializes session creation for **all users**. This violates pillar 1 (lock on hot path) and pillar 3 (hidden O(N) per op, no O(N) ack annotation). `insert_with_per_user_cap` runs on every successful SCRAM auth (`shamir-server/src/connection/handshake.rs:579`), so auth bursts hit a single global lock whose hold time grows linearly with total session count. Note also the O(N)-scan lock has only single-threaded integration coverage (`tests/integration_session.rs:381-448`); no concurrent-insert test exists.
- **Failure scenario:** server at 100k live sessions; a login storm (e.g. after a network partition heals) funnels every `auth_ok` through one mutex; each holder scans 100k entries for its ≤16 sessions while every other completing handshake blocks behind it — auth latency climbs with unrelated session population, and the lock converts a sharded concurrent map into a serialized one exactly when load is highest.
- **Suggested fix:** maintain a per-user secondary index (e.g. `DashMap<[u8;16], Vec<[u8; SESSION_ID_BYTES]>, FxBuild>` updated in insert/remove, or `scc::HashMap<user_id, tiny sid set>`), making the eviction scan O(≤cap) and shrinking the lock to per-user granularity (entry lock on the user's index slot). If a global lock is deliberately kept, add the mandated contention-model comment and an `// O(N) ack:` annotation per pillar 3. Also reconcile the cap gap: `process_resume` (`server/resume.rs:432`) uses the uncapped `insert`, so resumed sessions never count against `MAX_SESSIONS_PER_USER`.

### 3. `AuditChain::inner`: hot-path `parking_lot::Mutex` with no contention-model comment, and the critical section contains HMAC compute, string allocations, and a full entry clone
- **File:line:** `crates/shamir-connect/src/server/audit_chain.rs:131` (field), `196-215` (`append` critical section)
- **Severity:** medium
- **Issue:** `append` fires per audit event — i.e. per auth attempt/failure, per session eviction, per admin op — request-rate frequency, not setup-only. Per CLAUDE.md, a hot-path `parking_lot::Mutex` must carry an inline contention-model justification; the doc only says "Single-mutex inner state holds the next seq..." which describes the layout, not the contention model. Inside the lock the code allocates up to five `String`s (`event.into()` etc.), computes the entry HMAC (SHA-256 over the canonical bytes), then clones the whole entry to push into the in-memory vec — so hold time is dominated by heap churn + crypto, not by the seq/prev update that actually needs exclusivity. The lock is arguably defensible (a chain is a genuine linearization dependency: seq N+1's `prev_hmac` is seq N's `hmac`, awkward to express with two independent atomics), but neither the justification nor a minimized CS exists.
- **Failure scenario:** under an auth flood (each failure emits an audit event), concurrent appends serialize behind HMAC+allocation-sized critical sections; the audit mutex becomes a global throttle on the very path (auth) whose availability the spec's other defenses are protecting.
- **Suggested fix:** shrink the CS to `{(seq, prev_hmac) = state; state = (seq+1, placeholder)}`-style reserve/publish or an `AtomicU64` seq + `ArcSwap` published-hmac scheme where the HMAC, string materialization, and clone happen outside the lock; or keep the lock and add the mandated inline contention-model comment. Related pillar-2 exposure in the same path: `AuditChainWriter::append` (`audit_chain.rs:427`) invokes the sync `AuditAppender::append_entry` (implementations may write sqlite + fsync, per the trait doc "must fsync at least every 5 seconds") inline on the request thread — the crate offers no async wrapper or documented `spawn_blocking` contract, leaving that discipline to each caller.

### 4. `FjallConsumedCounters` doc claims the lock is "Not held across the fsync" — the code holds it across `persist(SyncAll)`
- **File:line:** `crates/shamir-connect/src/server/durable_counters.rs:51-53` (claim) vs `124-149` (guard spans `get` → `insert` → `persist(PersistMode::SyncAll)`)
- **Severity:** medium
- **Issue:** The `write_lock` field doc states "Not held across the fsync — fjall's `persist` is synchronous and short." The `_guard` acquired at line 124 lives to end of scope, so the `SyncAll` fsync at line 147 — typically milliseconds of disk latency — runs while holding the lock. The contention model itself is properly named and sound ("one call per session resumption", module doc §Atomicity), so this is doc/code drift, not a wrong design — but these inline contention-model comments are the crate's enforcement mechanism per CLAUDE.md (see the F-9 revision's history of exactly this kind of drift), and a future reader tuning resumption concurrency will trust the false claim.
- **Failure scenario:** none today beyond misdocumented serialization of concurrent resumes' fsyncs; risk is future code built on the incorrect statement.
- **Suggested fix:** correct the comment to say the lock *is* held across the (deliberately serialized) fsync, or restructure to release before persist if the get→insert→persist ordering permits; either way keep the named contention model accurate.

### 5. `Argon2Semaphore` exposes a blocking `Mutex`+`Condvar` wait for the auth path that no production caller uses — an executor-parking trap if adopted
- **File:line:** `crates/shamir-connect/src/server/argon2_semaphore.rs:20-21, 29-38, 84-110`
- **Severity:** low
- **Issue:** `acquire`/`acquire_until` block the calling thread (std `Mutex`+`Condvar` wait loop) and the module doc presents this as the design ("works for both sync and async Argon2 callers via a blocking wait()"). The only production consumer (`shamir-server`) exclusively uses the non-blocking `try_acquire` and routes real Argon2id through `spawn_blocking`; the blocking API has zero production callers (tests only). If a future caller invokes `acquire_until` from an async auth task once 64 permits are exhausted, tokio workers park on the condvar — a CLAUDE.md pillar-2 violation with the classic "SLOW/TIMEOUT under load" symptom the workspace explicitly hunts.
- **Failure scenario:** integrator reads the module doc, calls `sem.acquire()` before Argon2 in an async handler; at 64 concurrent derivations every worker thread blocks for up to the Argon2 duration → runtime starvation, request timeouts with no panic to point at.
- **Suggested fix:** either document the contract in bold on `acquire`/`acquire_until` ("sync context / `spawn_blocking` only — never call from an async task"), rename them accordingly (e.g. `acquire_blocking`), or delete the blocking surface until a caller needs it.

### 6. `lockout.rs` DashMaps use the default `RandomState` hasher — pillar-4 drift in the one file that keys on attacker-chosen subnets
- **File:line:** `crates/shamir-connect/src/server/lockout.rs:256-257`
- **Severity:** low
- **Issue:** CLAUDE.md pillar 4 makes the Fx hasher the workspace default for every hash-keyed structure. Every other concurrent map in this crate complies — `session.rs:410`, `rate_limit.rs:155`, `resume.rs:29`, `admin.rs:21` each alias `BuildHasherDefault<rustc_hash::FxHasher>` with a justification comment — but `InMemoryLockoutStore::failures`/`lockouts` are declared as plain `DashMap<PairKey, _>` and inherit SipHash/`RandomState`. `PairKey = (Subnet, [u8;16])` where the `Subnet` half is derived directly from the client-supplied IP. Practical DoS impact is small (SipHash is keyed per-process, so collision crafting isn't feasible), but the normative default is violated in exactly the module whose sibling already wrote the justification comment to copy.
- **Failure scenario:** none functional; 2–5× slower lookups on the failed-auth path and inconsistency with the documented workspace standard.
- **Suggested fix:** `type PairHasher = std::hash::BuildHasherDefault<rustc_hash::FxHasher>;` and `DashMap<PairKey, FailureState, PairHasher>` / `DashMap<PairKey, LockoutState, PairHasher>`, with the same DoS rationale comment used in `rate_limit.rs`.

### 7. `ServerIdentityState::rotate` is a non-atomic check-then-`store` on `ArcSwap` — concurrent admin rotates lose an update and strand the interim keypair
- **File:line:** `crates/shamir-connect/src/server/rotation.rs:151-180`
- **Severity:** low
- **Issue:** `rotate` loads the inner state, checks "not already in overlap", builds a new keypair, and `store()`s it — a read-check-write with no CAS. Two concurrent `rotateServerIdentity` calls both pass the pre-condition against the same loaded snapshot, both store, and last-writer-wins: the first rotated keypair is silently discarded while `current_version_atomic`/`current_version` end at the same value, and any `identity_sig`/ticket issued via `sign_with_current` inside the two-`store` window pins a key that is neither `current` nor `previous` afterward. `try_finalize` (lines 184-199) has the same load-clone-store shape but is idempotent (both racers write equivalent state), so only `rotate` matters. Frequency is admin-only, hence low.
- **Failure scenario:** double-invoked rotation (retry + original racing) yields clients that pinned the discarded keypair; their next handshake fails pin verification and the `rotation_in_progress` recovery payload is signed by the *original* previous key, not the one they pinned — orphan recovery breaks for that cohort.
- **Suggested fix:** re-load and `compare_and_swap`/`compare_exchange` (arc_swap) on the observed `Arc` inside a loop after building `new_inner`, retrying the pre-condition check on failure; or serialize admin ops upstream and say so in the doc.

### 8. Stale `Debug` label reports `permissions` as `"<RwLock>"`
- **File:line:** `crates/shamir-connect/src/server/session.rs:111`
- **Severity:** nit
- **Issue:** The custom `Debug` impl prints `permissions: "<RwLock>"`, but the field has been a plain `SessionPermissions` since the `parking_lot::RwLock` was removed (see the field's own doc at lines 143-148). Misleading when reading `{:?}` output during concurrency debugging.
- **Suggested fix:** change the label to `"<snapshot>"` or drop the field from the impl.

</details>

# shamir-connect -- Performance & O(x->0)

## Summary

The crate is largely exemplary on this axis — the documented "Optim #2..#9" work is real (pre-scheduled AES ciphers in `ResumeConfig`, zero-copy `RequestEnvelopeView`/`Ref` envelopes, `OnceLock`-cached per-session HMAC key, atomics-only `PostAuthBucket`, `FxHasher` on every fixed-size-key DashMap, exact-`Vec::with_capacity` canonical builders). Two genuine violations of pillar 3 remain, both on the hottest login/audit paths: the §7.4 per-user session-cap insert does a full O(total-sessions) map scan under a global mutex on every successful full auth, and `AuditChain` accumulates every audit event in an unbounded in-memory `Vec` for the life of the process. Additionally, the resumption path (the hottest session-creation path) inserts sessions without the per-user cap, so the uncapped path is O(1) while the capped one is O(N). Benches cover lookups/envelopes/crypto well but never exercise the capped insert or the audit append — exactly where both blind spots live.

## Findings

### 1. Per-user session-cap insert does an O(total-sessions) full-map scan under a global mutex on every login
- **File:line:** `crates/shamir-connect/src/server/session.rs:470-497` (scan loop 476-481; global lock 470; fields 415-417)
- **Severity:** high
- **Issue:** `SessionStore::insert_with_per_user_cap` — the spec §7.4 NORMATIVE LRU-cap enforcement that runs on every successful full SCRAM auth (production caller: `crates/shamir-server/src/connection/handshake.rs:579`) — iterates the **entire** `by_sid` DashMap (all sessions of **all** users) to collect the inserting user's sids, then sorts them. Cost per login is O(total live sessions), not O(cap)=O(16), and every insert across all users serializes behind the single `cap_lock: parking_lot::Mutex<()>`, which carries no inline contention-model justification (CLAUDE.md: "parking_lot::* banned in hot paths ... must be justified inline"). Net: total session-creation work is O(sessions × logins) and logins are globally serialized.
- **Failure scenario:** a busy server holding tens of thousands of live sessions pays a ~10k-entry map traversal per login plus lock handoff; a login burst stacks up behind one mutex, and each additional live session makes every future login slower.
- **Suggested fix:** keep a per-user index beside `by_sid` — e.g. `DashMap<[u8;16], Vec<[u8;32]>>` (or per-user `(sids, min-heap by last_activity_ns)`) updated at `insert`/`remove`/`kick`/GC — so LRU selection touches ≤ `max_sessions_per_user` entries; then drop `cap_lock` entirely or narrow it to the per-user shard.

### 2. AuditChain accumulates every audit event in an ever-growing in-memory Vec (unbounded growth)
- **File:line:** `crates/shamir-connect/src/server/audit_chain.rs:140` (`entries: Vec<AuditEntry>`), `:214` (`g.entries.push(entry.clone())`), also `:170-179` (`from_checkpoint` same shape)
- **Severity:** high
- **Issue:** every `append` unconditionally clones + pushes the entry into `ChainInner::entries`; there is no cap, no drain, no opt-out — the `AuditAppender` is an *additional* persistence sink, not a replacement, so the module doc's "production should override with a streaming writer" is not actually achievable via any API. Production wires exactly this (`crates/shamir-server/src/server/server_launcher.rs:339` creates the shared `AuditChain`). Each `AuditEntry` carries six `String`s, a `Vec`, and two 32-byte tags (hundreds of bytes), and audit events fire at least once per connection attempt (auth success/failure, rate-limits, evictions).
- **Failure scenario:** a long-running server accumulates millions of entries → RSS grows monotonically forever → eventual OOM; a memory leak by design. The periodic `checkpoint()` only persists `(next_seq, prev_hmac)` and never trims.
- **Suggested fix:** make the in-memory log opt-in for tests (e.g. `AuditChain::new_in_memory`), with the default chain keeping only `(next_seq, prev_hmac)` and handing entries straight to the appender; or keep a bounded ring (last N, documented constant). If kept, store `Arc<AuditEntry>` so the vec push is a refcount, not a deep clone.

### 3. Resume path bypasses MAX_SESSIONS_PER_USER — uncapped session creation on the hottest session-creation path
- **File:line:** `crates/shamir-connect/src/server/resume.rs:432` (`session_store.insert(session_id, session)`)
- **Severity:** medium
- **Issue:** `process_resume` mints and inserts a fresh `Session` per resumption via plain `insert`, never calling `insert_with_per_user_cap` — the only API enforcing the §7.4 NORMATIVE per-user cap (confirmed: `shamir-server`'s `run_resume` adds no cap enforcement after the call). So the cap+LRU machinery exists only on the full-auth path, and it is the O(N) one (finding 1), while the hotter reconnect path grows each user's session count unbounded between idle-GC ticks.
- **Failure scenario:** a client (legitimate multi-tab or a replaying stolen ticket) resumes every few seconds, each resume minting a new session id; per-user session entries and the global map grow until the external idle-GC task runs — amplifying finding 1's scan cost and skirting the spec cap.
- **Suggested fix:** route the insert through `insert_with_per_user_cap` inside `process_resume` (cheap once finding 1 is fixed) and return the `evicted` sid in `ResumeOk`-adjacent plumbing so callers can emit `session_evicted{reason="max_sessions_lru"}`.

### 4. AuditChain::append holds the chain mutex across allocations, HMAC, and a deep entry clone
- **File:line:** `crates/shamir-connect/src/server/audit_chain.rs:196-215`
- **Severity:** low
- **Issue:** one global `parking_lot::Mutex` serializes all audit emissions; that serialization is inherent (next entry's `prev_hmac` depends on this entry's `hmac`, and the doc says so), but the critical section also includes several `String` allocations (`event.into()` etc.), the `canonical_bytes` Vec build, HMAC-SHA256, and an O(entry) `entry.clone()` — all inside the lock.
- **Failure scenario:** under an auth burst, every audit emission queues behind the previous entry's HMAC+clone; latency adds up at high connection rates. Bounded impact (~µs each) — hence low.
- **Suggested fix:** shrink the locked work: compute `canonical_bytes` for the prev-independent prefix outside, or at minimum store `Arc<AuditEntry>` in the vec so the in-lock clone is a refcount bump.

### 5. Owning-envelope `dispatch_request` still pays a wall-clock syscall per request
- **File:line:** `crates/shamir-connect/src/server/dispatch.rs:78` (`store.lookup` → `Session::touch` → `UnixNanos::now()`, `session.rs:296-298`)
- **Severity:** nit
- **Issue:** the owning variant looks the session up via `lookup()`, which samples `UnixNanos::now()` internally (~100 ns syscall on Windows) per request; the zero-copy `dispatch_request_view` (Optim #4/#5) exists precisely to avoid this and is the documented hot path, yet the non-view variant remains exported and is benched as `dispatch/happy_path` (`benches/hot_paths.rs:234`), inviting use on hot transports.
- **Failure scenario:** none — constant small per-request overhead.
- **Suggested fix:** doc-mark `dispatch_request` as the non-hot/compat entry point, or thread a caller-captured `now_ns` through it so both variants amortize the clock read.

### Test-coverage note (claims check)
Unit coverage is thorough for the hot paths that matter (`src/server/tests/`: session, rate_limit, post_auth_rate_limit, lockout, audit_chain, durable_counters, argon2_semaphore, changepw_challenge; `src/common/tests/`: 12 topic files; 9 `tests/integration_*.rs`). `insert_with_per_user_cap` has behavioral LRU tests (`tests/integration_session.rs:381-448`) but nothing at scale, and `benches/hot_paths.rs` benches only raw lookup/validity/hmac_key — neither the capped insert nor `AuditChain::append` is measured, which is consistent with findings 1-2 going unnoticed.

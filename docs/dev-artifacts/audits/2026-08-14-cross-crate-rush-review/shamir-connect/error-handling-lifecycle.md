<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-connect — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Connect still conflates counter errors with replay and has wiping, corruption-decoding and TTL gaps. Exact fjall inspection changes the outage model materially: failed persistence publishes no successful resume, logs upstream and poisons the whole counter database. Blanket silence and permanent-family durability claims are incorrect.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 15 | 11 | 0 | 0 | 2 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — FjallConsumedCounters::try_advance conflates persistence failure with replay and logs nothing

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Connect's bool result loses get/insert/persist causes and adds no diagnostics. Exact fjall 3.1.6 publishes the successful insert before persist; persist failure logs and poisons the database, preventing later family writes. Thus total operator silence is false. Source: https://docs.rs/crate/fjall/3.1.6/source/src/db.rs and src/keyspace/mod.rs.

Evidence: [crates/shamir-connect/src/server/durable_counters.rs:128](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/durable_counters.rs#L128); [crates/shamir-connect/src/server/durable_counters.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/durable_counters.rs#L140); [crates/shamir-connect/src/server/durable_counters.rs:147](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/durable_counters.rs#L147); [crates/shamir-connect/src/server/resume.rs:366](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L366); [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332).

<a id="review-1-persistence-visibility"></a>

### Claim 1.persistence-visibility — Failed fsync makes the advanced counter durable-visible and permanently bricks the family

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `unverified`.

Exact fjall 3.1.6 establishes in-process visibility, not guaranteed durability after failed sync_all. It logs and poisons the database, so failure scope is broader than one lineage; restart survival is uncertain and permanent family loss is not established. Published source: https://docs.rs/crate/fjall/3.1.6/source/src/db.rs, src/keyspace/mod.rs and src/journal/writer.rs.

Evidence: [crates/shamir-connect/src/server/durable_counters.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/durable_counters.rs#L140); [crates/shamir-connect/src/server/durable_counters.rs:147](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/durable_counters.rs#L147); [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332).

<a id="review-2"></a>

### Claim 2 — Client password slices are not zeroized on the error path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No scope-exit guard wipes either password on every return. Invalid challenge parameters/nonces, invalid new-password policy and failed old/new derivation can return before the applicable wipe.

Evidence: [crates/shamir-connect/src/client/handshake.rs:209](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L209); [crates/shamir-connect/src/client/bootstrap.rs:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/bootstrap.rs#L93); [crates/shamir-connect/src/client/changepw.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/changepw.rs#L43); [crates/shamir-connect/src/client/changepw.rs:71](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/changepw.rs#L71).

<a id="review-3"></a>

### Claim 3 — Subtraction underflow on backwards clock in changePassword TTL check

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The live verifier consumes the challenge and then subtracts two u64 wall-clock samples without checking order. Backwards time can panic with overflow checks or reject after wrapping; it does not bypass proof verification.

Evidence: [crates/shamir-connect/src/server/changepw.rs:139](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/changepw.rs#L139); [crates/shamir-connect/src/server/changepw.rs:141](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/changepw.rs#L141); [crates/shamir-server/src/db_handler/admin.rs:517](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L517).

<a id="review-4"></a>

### Claim 4 — dispatch_request lacks the rate gate; dispatch rate_limited branch untested

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Owned dispatch omits token enforcement, and registered connect dispatch integrations never exhaust the bucket and assert the rate_limited envelope plus non-invocation of the handler.

Evidence: [crates/shamir-connect/src/server/dispatch.rs:100](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/dispatch.rs#L100); [crates/shamir-connect/src/server/dispatch.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/dispatch.rs#L153); [crates/shamir-connect/tests/integration_session.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/tests/integration_session.rs#L162).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — AuditAppender persistence failures are unreportable to AuditChainWriter

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Void methods prevent writer-level success/error handling after chain advancement. The shipped sink logs failures; its checkpoint method nevertheless attempts checkpoint persistence after a failed flush, so logging is not durable-state coordination.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:344](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L344); [crates/shamir-connect/src/server/audit_chain.rs:417](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L417); [crates/shamir-server/src/audit_appender.rs:709](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/audit_appender.rs#L709); [crates/shamir-server/src/audit_appender.rs:712](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/audit_appender.rs#L712).

<a id="review-6"></a>

### Claim 6 — OS RNG failures panic instead of returning Result

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both wrappers convert fallible OS entropy into panic. A separate environmental failure is required; normal nonce input does not trigger it.

Evidence: [crates/shamir-connect/src/common/crypto.rs:69](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/crypto.rs#L69); [crates/shamir-connect/src/common/crypto.rs:219](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/crypto.rs#L219).

<a id="review-7"></a>

### Claim 7 — FjallConsumedCounters::open exposes fjall::Error

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The optional explicitly Fjall-specific backend returns a typed upstream error. This is prospective API coupling, not an incorrect error channel or demonstrated semantic defect; the package is unpublished. A local wrapper is optional design work.

Evidence: [crates/shamir-connect/src/server/durable_counters.rs:70](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/durable_counters.rs#L70); [crates/shamir-connect/Cargo.toml:4](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/Cargo.toml#L4); [crates/shamir-connect/Cargo.toml:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/Cargo.toml#L20).

<a id="review-8"></a>

### Claim 8 — unpack_value panics on malformed persisted counter values

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Short values reaching peek/try_advance panic at slices; longer values silently ignore trailing bytes. GC alone validates exact length. Reachability requires malformed storage/legacy state, not a normal remotely supplied ticket value.

Evidence: [crates/shamir-connect/src/server/durable_counters.rs:97](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/durable_counters.rs#L97); [crates/shamir-connect/src/server/durable_counters.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/durable_counters.rs#L110); [crates/shamir-connect/src/server/durable_counters.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/durable_counters.rs#L130); [crates/shamir-connect/src/server/durable_counters.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/durable_counters.rs#L163).

<a id="review-9"></a>

### Claim 9 — process_resume reparses transport with a silent fail-open default

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

validate_ticket_enums rejects unknown transport before the fallback, and the plaintext byte is unchanged. Removing validation in a hypothetical mutation is not present reachability.

Evidence: [crates/shamir-connect/src/server/resume.rs:276](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L276); [crates/shamir-connect/src/server/resume.rs:389](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L389); [crates/shamir-connect/src/server/ticket.rs:279](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/ticket.rs#L279).

<a id="review-10"></a>

### Claim 10 — validate_client_kdf_safe returns a stringly-typed error

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The String-returning public helper remains; prior stricter handshake validation excludes its outer-cap failure.

Evidence: [crates/shamir-connect/src/common/kdf_params.rs:77](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/kdf_params.rs#L77); [crates/shamir-connect/src/client/handshake.rs:209](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L209).

Grouping/duplicate: [api-wire-protocol.md#8](api-wire-protocol.md#review-8). This is not an additional independent defect.

<a id="review-11"></a>

### Claim 11 — Error::to_wire is unused and its collapse set untested

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

No helper call or direct collapse-set assertion exists. Inspected auth failures are explicitly collapsed and malformed envelopes are separately mapped; dead-helper cleanup is not evidence of a live authentication leak.

Evidence: [crates/shamir-connect/src/common/error.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/error.rs#L89); [crates/shamir-connect/src/server/resume.rs:272](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L272); [crates/shamir-server/src/connection/request_loop.rs:383](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L383).

<a id="review-12"></a>

### Claim 12 — Consolidated error-path test gaps

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Registered snapshot sinks always succeed; no injected durable-counter error, all-exit password-buffer assertion or exact backwards-clock TTL oracle was found. The close/reopen counter test is not a power-loss test.

Evidence: [crates/shamir-connect/src/server/tests/rate_limit_tests.rs:143](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/tests/rate_limit_tests.rs#L143); [crates/shamir-connect/src/server/tests/lockout_tests.rs:187](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/tests/lockout_tests.rs#L187); [crates/shamir-connect/src/server/tests/durable_counters_tests.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/tests/durable_counters_tests.rs#L47); [crates/shamir-connect/tests/integration_changepw.rs:417](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/tests/integration_changepw.rs#L417).

<a id="review-13"></a>

### Claim 13 — AuditError is hand-rolled with Display equal to Debug

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Manual Debug-based Display remains a house-style/diagnostic-format issue, not incorrect propagation.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:328](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L328); [crates/shamir-connect/src/server/audit_chain.rs:334](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L334).

<a id="review-non-findings-invariant-expects"></a>

### Claim Non-findings.invariant-expects — Programmer-invariant expect calls

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The token closure always returns Some and password's first-character expect follows nonempty validation. Exact hmac 0.12.1 HmacCore::new_from_slice returns Ok for arbitrary key lengths; https://docs.rs/crate/hmac/0.12.1/source/src/optim.rs.

Evidence: [crates/shamir-connect/src/server/session.rs:381](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/session.rs#L381); [crates/shamir-connect/src/common/password.rs:42](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/password.rs#L42); [crates/shamir-connect/src/common/crypto.rs:122](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/crypto.rs#L122); [Cargo.lock:1638](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1638).

## Evidence and recipe corrections

- Replace the permanent-family/full-SCRAM-recovery story: fjall 3.1.6 persistence failure poisons the counter database, so newly issued families also cannot advance until storage recovery/reopen.
- An unconditional rollback cannot preserve uncertain durability and is additionally rejected by the poisoned database. Keep replay protection fail-closed and expose the storage outage.
- The malformed-value recipe must reject corruption, not treat it as no prior counter.
- A Result-returning audit sink is insufficient without serialized append/checkpoint ordering and a policy preventing checkpoint advancement past failed durable work.
- AUTH_PROTOCOL currently says self-service password rotation was removed from v1, while the shipped handler still invokes it. Existing section citations do not establish a consistent frozen-v1 lifecycle contract.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-connect -- Error handling & resource lifecycle

## Summary

The crate is largely on-convention: a single thiserror `Error` enum with wire-privacy collapse, `Result` propagation throughout, RAII `Argon2Permit`, and logged/propagated snapshot-sink errors in lockout/rate-limit. The real gaps concentrate in the error paths themselves: the durable replay counter silently mutates state before durability is confirmed and conflates storage failure with replay (with zero logging), client flows skip the documented password zeroize when derivation fails, a wall-clock subtraction can underflow-panic on an NTP step-back, and the twin dispatch entry points enforce the post-auth rate-limit error path in only one of the two. Several error APIs are stringly-typed or dead (`to_wire` has no callers workspace-wide), and the associated error-path tests are absent exactly where the bugs are.

## Findings

### 1. `FjallConsumedCounters::try_advance` conflates persistence failure with replay, mutates state before durability, and logs nothing
- **File:line:** `crates/shamir-connect/src/server/durable_counters.rs:115-151` (get err → `false` at 128; insert at 140; persist check at 147-149); trait contract at `src/server/resume.rs:40-55`
- **Severity:** high
- **Issue:** The `ConsumedCounterStore::try_advance` trait returns `bool`, so the only durable implementation must fold three very different outcomes — read error, insert error, fsync error — into the same `false` that means "replay/stale" upstream (`process_resume` maps it to generic `Error::AuthFailed`). Worse, `keyspace.insert` (line 140) lands the new counter in fjall's in-memory journal *before* `persist(PersistMode::SyncAll)` is attempted (line 147); on persist failure the method returns `false` but the advanced counter is already visible to every later read. There is also no `log::warn!` anywhere in `try_advance` (contrast `gc` at lines 178/183, and the lockout/rate-limit snapshot paths) despite `Cargo.toml:46-48` adding the `log` dependency precisely because persistence failures "must surface to operators".
- **Failure scenario:** Transient I/O failure at exactly the fsync step: the client's resume fails with `authentication_failed`, the journalled counter is nonetheless durable-visible, and the client's retry with the same ticket now hits `new_counter > c == false` — the ticket family is permanently bricked (recovery only via full SCRAM re-auth). Meanwhile operators see silent auth-failure spikes with no diagnostic; a read-error outage is indistinguishable from a replay attack.
- **Suggested fix:** Log every error branch in `try_advance` at `warn`/`error` (matching `gc`). Change the trait to return an enum or `Result` (e.g. `Accepted / Replayed / StorageError`) so `process_resume` can fail with `ServerBusy`-style semantics instead of `AuthFailed` on storage trouble, and so a persist failure can at least be distinguished. Where the trait shape must stay `bool` for compatibility, best-effort roll back the journalled insert on persist failure and log loudly. Add fault-injection tests (wrapper around the db handle) pinning the post-failure behavior; today `durable_counters_tests.rs` covers only happy paths and restart durability.

### 2. Client password slices are not zeroized on the error path, contradicting their doc contracts
- **File:line:** `crates/shamir-connect/src/client/handshake.rs:231-232`; `crates/shamir-connect/src/client/bootstrap.rs:96-97`; `crates/shamir-connect/src/client/changepw.rs:60-61 and 71-72`
- **Severity:** medium
- **Issue:** All three flows call `DerivedKeys::derive(password, ...)?` and only then `password.zeroize()`. The `?` returns early on derivation failure, skipping the zeroize. `ClientHandshake::process_challenge`'s doc says "`password` is consumed and zeroized on return" — unconditional on its face; `build_request` in bootstrap/changepw similarly promise zeroize "after use". The error path is reachable: `validate_client_limits` (`common/kdf_params.rs:34-43`) checks only *upper* bounds, so a misbehaving/compromised server can send degenerate params (e.g. `memory_kb < 8*parallelism`) that pass validation and then fail inside `argon2id` → `Params::new` / `hash_password_into` (`common/crypto.rs:158-165`).
- **Failure scenario:** A hostile server probes a client with degenerate KDF params; Argon2id fails; the client's raw password bytes remain unscrubbed in the caller's buffer, violating the secret-hygiene contract the docs advertise and undermining the crate's otherwise strict zeroization discipline (Zeroizing keys, redacted Debug impls).
- **Suggested fix:** Zeroize before every fallible step after the password is consumed (or wrap the body in a closure/guard that zeroizes on all exits), so the doc contract holds on both paths. Add an error-path test using degenerate `KdfParams` that exercises the derive-failure branch — it currently has no coverage at all.

### 3. Signed-subtraction underflow panic on a backwards clock step in `changePassword` TTL check
- **File:line:** `crates/shamir-connect/src/server/changepw.rs:141`
- **Severity:** medium
- **Issue:** `if now_ns - pending.issued_at_ns > CHANGEPW_CHALLENGE_TTL_NS` subtracts two independently-sampled wall-clock values (`UnixNanos` is `SystemTime`-based, `common/time.rs:18-23`, documented as "NTP-disciplined"). If `now_ns` is earlier than `issued_at_ns` — an NTP step-back between challenge issue and verify, or any caller passing a stale/mock clock — the `u64` subtraction underflows: panic in debug builds, huge wrapped value in release (spurious `AuthFailed`).
- **Failure scenario:** Server clock steps backwards ~1 s after a user requests a change-password challenge; the user's next `changePassword` submit panics the request task in a debug build (or is silently rejected in release), with the challenge already consumed by the atomic `swap(None)` at line 139.
- **Suggested fix:** `now_ns.saturating_sub(pending.issued_at_ns)` (or an explicit `now_ns < issued` early-return). Extend `tests/integration_changepw.rs::rejects_after_ttl_expiration` with a clock-regression case (`now_ns < issued_at_ns`) — the current suite only ever tests forward time.

### 4. `dispatch_request` lacks the post-auth rate-limit gate that its twin `dispatch_request_view` enforces; the `rate_limited` error branch is untested
- **File:line:** `crates/shamir-connect/src/server/dispatch.rs:69-110` (no gate) vs `150-160` (gate); both exported (`dispatch_request` re-exported at `src/server/mod.rs:28`)
- **Severity:** medium
- **Issue:** `dispatch_request_view` applies the task-#608 per-session token bucket and returns the `rate_limited` error envelope; the non-view `dispatch_request` — the variant re-exported from `server/mod.rs` — silently does not, even though its doc describes the same steps 1-5. Two public entry points with divergent error-path enforcement is a trap for transport integrators (shamir-server uses only the view variant, so production is covered today).
- **Failure scenario:** A new transport binding wired to the documented `dispatch_request` gets no post-auth rate limiting at all; per-session request floods are only caught by the connection-level in-flight cap, if any.
- **Suggested fix:** Apply the same gate in `dispatch_request` (hoist into a shared helper), or delete/deprecate the non-view variant so there is exactly one choke point. Add a dispatch-level test that a drained bucket yields the `rate_limited` envelope — no test currently covers that branch of either function (existing coverage is only at the `Session::check_post_auth_rate_limit` unit level).

### 5. `AuditAppender` persistence failures are unreportable by design; audit chain advances silently ahead of durable storage
- **File:line:** `crates/shamir-connect/src/server/audit_chain.rs:341-348` (trait methods return `()`), `406-434` (`AuditChainWriter::append` calls the appender unconditionally)
- **Severity:** medium
- **Issue:** `AuditAppender::append_entry`/`checkpoint` have no error channel, and `AuditChainWriter` updates the in-memory chain (`seq`, `prev_hmac`) *before/independently* of the appender result. A failing durable appender (disk full, fsync error) loses audit events with no signal, and the truncation-defence checkpoint can then describe chain state that never reached storage — or not be written at all, equally silently. The crate's own `Cargo.toml:46-48` rationale says such failures "must surface to operators"; lockout/rate-limit snapshot sinks got `Result` + `log`, the audit path did not.
- **Failure scenario:** Disk-full during an incident: audit events are dropped and no `log` line, metric, or error surfaces; at restart, chain verification either flags a puzzling truncation or the gap is never noticed.
- **Suggested fix:** Give `append_entry`/`checkpoint` a `Result<(), AuditError-or-sink-error>` return (or at minimum have `AuditChainWriter` log on failure via the `log` crate), and add a failing-appender test asserting the failure is observable. (Related nit below: `AuditError` itself is hand-rolled.)

### 6. OS RNG failures panic instead of returning `Result`
- **File:line:** `crates/shamir-connect/src/common/crypto.rs:66-70` (`random_bytes`), `215-219` (`Ed25519Keypair::generate`)
- **Severity:** low
- **Issue:** `.expect("OS RNG failure")` converts an environment failure (not a programmer-invariant violation) into a panic, against the house rule "Return `Result<T, E>`; avoid `panic!` outside `unreachable!()`/invariant violations". These functions sit on the nonce/session-id/ticket-family generation paths of every handshake and resume.
- **Failure scenario:** On a hardened system where `getrandom` can fail (seccomp filter, exhausted entropy edge, container misconfiguration), every connection task panics rather than surfacing a `ServerBusy`-class error. Practically near-unreachable on supported platforms, hence low.
- **Suggested fix:** Return `Result` variants (e.g. `try_random_bytes`), or keep the panic but annotate it as a deliberate unrecoverable-system-state invariant so it is visibly excepted from the house rule.

### 7. `FjallConsumedCounters::open` leaks the third-party `fjall::Error` into the library API
- **File:line:** `crates/shamir-connect/src/server/durable_counters.rs:70`
- **Severity:** low
- **Issue:** Returns `Result<Self, fjall::Error>`, coupling callers to an optional-dependency's error type (`durable-fjall` feature) instead of the crate's `Error`/a local thiserror type — the only public API in the crate whose error type is neither.
- **Failure scenario:** Callers must match on `fjall::Error` (and add `fjall` to their deps to name it) to classify startup failures; a future storage-backend swap becomes a breaking change.
- **Suggested fix:** Wrap in a small `#[derive(thiserror::Error)]` enum (a `#[from] fjall::Error` variant is exactly the "where natural" case CLAUDE.md calls for).

### 8. `unpack_value` panics on a malformed persisted counter value; only `gc` guards the length
- **File:line:** `crates/shamir-connect/src/server/durable_counters.rs:94-100` (unguarded slicing); unguarded call sites at 110 (`peek`) and 130 (`try_advance`); guard exists only in `gc` at 163
- **Severity:** low
- **Issue:** `unpack_value` indexes `v[8..16]` without a length check. Any truncated/corrupt 16-byte value on disk (external corruption, partial legacy write) makes `try_advance` — an authentication-path function — panic via `copy_from_slice` instead of failing closed.
- **Failure scenario:** One corrupt key in the counters keyspace turns every resume attempt for that (user, family) into a task panic rather than a rejection.
- **Suggested fix:** Return `Option<(u64, u64)>`/`Result` from `unpack_value` on `v.len() != 16` and treat as "no prior" plus a `log::warn!`, mirroring `gc`'s defensive skip.

### 9. `process_resume` discards the validated transport enum and re-derives it with a silent fail-open default
- **File:line:** `crates/shamir-connect/src/server/resume.rs:276-277` (validated tuple discarded: `_transport_at_auth`), `388-389` (`TransportKind::from_u8(...).unwrap_or(TransportKind::Tcp)`)
- **Severity:** low
- **Issue:** `validate_ticket_enums` already fails-closed on an unknown `transport_kind_at_auth`, but its transport half is thrown away and the raw byte is re-parsed at line 389 with `.unwrap_or(Tcp)`. Today the fallback is dead; if validation is ever reordered/loosened, an unknown enum silently becomes `Tcp` — a fail-open default on a security-relevant field, in a function whose documented posture is "any failure → `Error::AuthFailed`".
- **Failure scenario:** A future edit drops or moves the `validate_ticket_enums` call; tickets with garbage transport bytes resume successfully as `Tcp` sessions with no error anywhere.
- **Suggested fix:** Use the `(transport, binding)` tuple returned by `validate_ticket_enums` and delete the `unwrap_or` re-parse, making the fail-closed guarantee structural.

### 10. `validate_client_kdf_safe` returns a stringly-typed error in a public library API
- **File:line:** `crates/shamir-connect/src/common/kdf_params.rs:77-91`
- **Severity:** low
- **Issue:** `Result<(), String>` contradicts the house rule (thiserror enum / crate `Error` for library APIs, no ad-hoc error types). The only in-crate caller immediately discards the message (`client/handshake.rs:210`, `if let Err(_msg)`), so the diagnostic content is dead weight while callers cannot match on the failure kind.
- **Failure scenario:** An embedder wants to distinguish "server requested oversized KDF" from other failures to warn the user; they must substring-match a `String`.
- **Suggested fix:** Return `crate::Error::KdfParamsRejected` (the caller already maps it to exactly that) or a small thiserror enum with the two limit kinds.

### 11. `Error::to_wire` — the wire-privacy collapse — is dead code workspace-wide and untested
- **File:line:** `crates/shamir-connect/src/common/error.rs:86-103` (no callers anywhere in the workspace)
- **Severity:** low
- **Issue:** The helper encoding the spec §14.4 rule ("any internal cause collapses to generic `AuthFailed` on the wire") has zero call sites; the discipline is instead maintained ad hoc by `map_err(|_| Error::AuthFailed)` chains (e.g. all of `process_resume`) while `dispatch_request*` propagate internal `Error::InvalidInput("...")` detail strings to the transport layer, which must remember to collapse them itself. Nothing tests the collapse set (which variants survive vs. become `AuthFailed`).
- **Failure scenario:** A future path forwards an internal error (with its distinguishing message/variant) straight into an `ErrorEnvelope`; no helper, type, or test stands in the way.
- **Suggested fix:** Either route the transport-boundary error mapping through `to_wire` (and unit-test the preserved set: `RateLimited`/`ServerBusy`/`UnsupportedVersion`/`BootstrapFailed`) or delete it and document the per-call-site `map_err` convention explicitly.

### 12. Consolidated error-path test gaps
- **File:line:** various (see below)
- **Severity:** low
- **Issue:** Beyond the gaps already noted per finding (durable-counter failures #1, zeroize-on-error #2, clock regression #3, dispatch `rate_limited` branch #4, failing audit appender #5): (a) no test exercises `LockoutSnapshotSink::save`/`RateLimitSnapshotSink::save` *failure* propagation through `persist_snapshot` (only success and no-sink paths in `lockout_tests.rs:362-383`, `rate_limit_tests.rs:207-238`); (b) no client-side test drives the Argon2-failure branch of `DerivedKeys::derive` via degenerate KDF params, so none of the error paths in findings #2/#9 is pinned by the suite.
- **Failure scenario:** Regressions in exactly these branches (the ones that touch storage, secrets, or fail-open defaults) would land green.
- **Suggested fix:** Add fault-injection sinks (failing `save`) and degenerate-param cases to the respective test files; both need no new production code.

### 13. `AuditError` is hand-rolled instead of thiserror, with `Display` = `Debug`
- **File:line:** `crates/shamir-connect/src/server/audit_chain.rs:297-334`
- **Severity:** nit
- **Issue:** Manual `Display` that just forwards `{:?}` plus an empty `std::error::Error` impl, where every comparable error in the crate (`Error`, `LockoutSnapshotError`, `RateLimitSnapshotError`) uses `thiserror` with human-readable messages per CLAUDE.md.
- **Failure scenario:** Operator-facing output renders `SequenceGap { at: 3, expected: 4, found: 7 }` instead of a sentence; field additions silently change log format.
- **Suggested fix:** Convert to `#[derive(Debug, thiserror::Error)]` with per-variant `#[error("...")]` strings.

---
Reviewed files: all 43 `src/**/*.rs` under `crates/shamir-connect/` (incl. `src/server/tests/`, `src/common/tests/` manifests and per-topic files), `Cargo.toml`, `tests/integration_*.rs`, and `benches/hot_paths.rs` (skim); cross-checked callers in `crates/shamir-server` for `dispatch_request*`, `to_wire`, `FjallConsumedCounters`, and snapshot persistence. Non-findings verified as convention-compliant: `session.rs:381` / `crypto.rs:122` / `password.rs:42` `expect`s are genuine programmer-invariant cases (closure-always-`Some`, fixed-key-length HMAC, non-empty-after-min-length) and are acceptable under the documented rule.

</details>

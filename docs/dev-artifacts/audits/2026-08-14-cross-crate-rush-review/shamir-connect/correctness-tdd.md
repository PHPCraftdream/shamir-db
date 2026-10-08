<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-connect — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Most concrete source defects remain. Correct the TOFU threat model, auth-message arithmetic, and TTL coverage claims; existing tests do not establish an entirely green suite.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 19 | 19 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — TOFU pin callback fires before Ed25519 identity verification completes

Status: `confirmed-open`. Current risk: `medium`.

The callback still runs before strict verification and can persist an unverified pin in an embedder. Shipped Rust clients capture it locally and expose it only after success. The persistent-MITM scenario additionally needs relay-compatible binding; ordinary first-use TOFU already permits an attacker who supplies a valid signature with its own key.

Evidence: [crates/shamir-connect/src/client/handshake.rs:274](../../../../../crates/shamir-connect/src/client/handshake.rs#L274); [crates/shamir-connect/src/client/handshake.rs:288](../../../../../crates/shamir-connect/src/client/handshake.rs#L288); [crates/shamir-client/src/client.rs:615](../../../../../crates/shamir-client/src/client.rs#L615); [crates/shamir-client/src/client.rs:626](../../../../../crates/shamir-client/src/client.rs#L626).

<a id="review-2"></a>

### Claim 2 — Rate-limiter refill watermark can regress — the exact #1090 hazard, unfixed at the sibling site

Status: `confirmed-open`. Current risk: `medium`.

Elapsed time saturates, but the subsequent unconditional watermark assignment permits an older timestamp to move the bucket backwards and a later call to re-credit time.

Evidence: [crates/shamir-connect/src/server/rate_limit.rs:339](../../../../../crates/shamir-connect/src/server/rate_limit.rs#L339); [crates/shamir-connect/src/server/rate_limit.rs:342](../../../../../crates/shamir-connect/src/server/rate_limit.rs#L342); [crates/shamir-server/src/connection/handshake.rs:680](../../../../../crates/shamir-server/src/connection/handshake.rs#L680).

<a id="review-3"></a>

### Claim 3 — verify_against_checkpoint flags a legitimately stale checkpoint as truncation

Status: `confirmed-open`. Current risk: `medium`.

The verifier still accepts only two terminal sequence relationships, rejecting a valid log extended beyond its periodic checkpoint. Production startup does not call this helper; registered tests cover aligned and truncated logs, not a stale checkpoint.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:277](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L277); [crates/shamir-connect/src/server/audit_chain.rs:429](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L429); [crates/shamir-connect/src/server/tests/audit_chain_tests.rs:121](../../../../../crates/shamir-connect/src/server/tests/audit_chain_tests.rs#L121); [crates/shamir-connect/src/server/tests/audit_chain_tests.rs:136](../../../../../crates/shamir-connect/src/server/tests/audit_chain_tests.rs#L136).

<a id="review-4"></a>

### Claim 4 — client feature does not imply server, yet client code imports crate::server::* unconditionally

Status: `confirmed-open`. Current risk: `medium`.

Independent feature declarations and server-gated module registration conflict with unconditional client imports. This is source-level configuration proof, not an executed compiler result.

Evidence: [crates/shamir-connect/Cargo.toml:18](../../../../../crates/shamir-connect/Cargo.toml#L18); [crates/shamir-connect/src/lib.rs:29](../../../../../crates/shamir-connect/src/lib.rs#L29); [crates/shamir-connect/src/client/handshake.rs:27](../../../../../crates/shamir-connect/src/client/handshake.rs#L27); [crates/shamir-connect/src/client/bootstrap.rs:17](../../../../../crates/shamir-connect/src/client/bootstrap.rs#L17).

Grouping/duplicate: `api-wire-protocol.md#1`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — dispatch_request silently omits the #608 post-auth rate-limit gate its functionally identical twin has

Status: `confirmed-open`. Current risk: `medium`.

Owned dispatch proceeds directly to the handler; only view dispatch consumes a rate token. The shipped server uses view dispatch, so the bypass concerns alternate integrations, not its current request loop.

Evidence: [crates/shamir-connect/src/server/dispatch.rs:100](../../../../../crates/shamir-connect/src/server/dispatch.rs#L100); [crates/shamir-connect/src/server/dispatch.rs:153](../../../../../crates/shamir-connect/src/server/dispatch.rs#L153); [crates/shamir-connect/src/server/mod.rs:28](../../../../../crates/shamir-connect/src/server/mod.rs#L28); [crates/shamir-server/src/connection/request_loop.rs:340](../../../../../crates/shamir-server/src/connection/request_loop.rs#L340).

Grouping/duplicate: `security-crypto.md#1`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — ServerIdentityState::rotate / try_finalize are load-clone-store races despite the Atomic doc

Status: `confirmed-open`. Current risk: `low`.

Both mutators still decide from a snapshot then store without CAS. Concurrent rotations can lose a key; finalization of an expired snapshot can overwrite a subsequent rotation. No live server rotation caller was found, although its finalizer is registered.

Evidence: [crates/shamir-connect/src/server/rotation.rs:152](../../../../../crates/shamir-connect/src/server/rotation.rs#L152); [crates/shamir-connect/src/server/rotation.rs:169](../../../../../crates/shamir-connect/src/server/rotation.rs#L169); [crates/shamir-connect/src/server/rotation.rs:185](../../../../../crates/shamir-connect/src/server/rotation.rs#L185); [crates/shamir-connect/src/server/rotation.rs:194](../../../../../crates/shamir-connect/src/server/rotation.rs#L194); [crates/shamir-server/src/scheduler.rs:233](../../../../../crates/shamir-server/src/scheduler.rs#L233).

Grouping/duplicate: `concurrency-lockfree.md#7`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — Password buffers are not zeroized on early-error returns, violating the documented contract

Status: `confirmed-open`. Current risk: `low`.

Validation and derivation can return before explicit wiping in all three client flows. Over-limit parameters and zero server nonce provide source-proven early exits; recovery of those bytes requires a separate memory-disclosure capability.

Evidence: [crates/shamir-connect/src/client/handshake.rs:209](../../../../../crates/shamir-connect/src/client/handshake.rs#L209); [crates/shamir-connect/src/client/handshake.rs:232](../../../../../crates/shamir-connect/src/client/handshake.rs#L232); [crates/shamir-connect/src/client/bootstrap.rs:93](../../../../../crates/shamir-connect/src/client/bootstrap.rs#L93); [crates/shamir-connect/src/client/changepw.rs:43](../../../../../crates/shamir-connect/src/client/changepw.rs#L43).

Grouping/duplicate: `error-handling-lifecycle.md#2`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — changePassword TTL check underflows on clock regression; TTL boundary untested

Status: `confirmed-open`. Current risk: `low`.

Unsigned subtraction remains unchecked after challenge consumption. Clock regression can panic with overflow checking or wrap otherwise. A forward-expiration integration test exists; exact boundary and backwards-clock cases remain absent.

Evidence: [crates/shamir-connect/src/server/changepw.rs:139](../../../../../crates/shamir-connect/src/server/changepw.rs#L139); [crates/shamir-connect/src/server/changepw.rs:141](../../../../../crates/shamir-connect/src/server/changepw.rs#L141); [crates/shamir-connect/tests/integration_changepw.rs:195](../../../../../crates/shamir-connect/tests/integration_changepw.rs#L195).

Grouping/duplicate: `error-handling-lifecycle.md#3`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — encode_details_canonical is a placeholder that encodes nothing, with an unusable signature

Status: `confirmed-open`. Current risk: `low`.

The public helper still accepts serializer configurations rather than values, ignores the map, and returns empty bytes. No caller was found; current production audit emission is not using it.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:355](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L355); [crates/shamir-connect/src/server/audit_chain.rs:359](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L359).

Grouping/duplicate: `api-wire-protocol.md#3`. This row is not another independent defect.

<a id="review-10"></a>

### Claim 10 — Vacuous variant assertions in the all-zero nonce tests

Status: `confirmed-open`. Current risk: `low`.

Both registered tests discard matches! results. unwrap_err detects acceptance, but not a wrong error variant.

Evidence: [crates/shamir-connect/src/common/tests/auth_message_tests.rs:151](../../../../../crates/shamir-connect/src/common/tests/auth_message_tests.rs#L151); [crates/shamir-connect/src/common/tests/auth_message_tests.rs:175](../../../../../crates/shamir-connect/src/common/tests/auth_message_tests.rs#L175); [crates/shamir-connect/src/common/tests/mod.rs:1](../../../../../crates/shamir-connect/src/common/tests/mod.rs#L1).

<a id="review-11"></a>

### Claim 11 — InMemoryRateLimiter::with_rate(_, 0) breaks the limiter after warmup

Status: `confirmed-open`. Current risk: `medium`.

Zero is accepted and returned after warmup. A new bucket subtracts one token from zero capacity; an existing empty bucket divides by zero. The release path is not reliably unlimited: subsequent checks can panic.

Evidence: [crates/shamir-connect/src/server/rate_limit.rs:203](../../../../../crates/shamir-connect/src/server/rate_limit.rs#L203); [crates/shamir-connect/src/server/rate_limit.rs:324](../../../../../crates/shamir-connect/src/server/rate_limit.rs#L324); [crates/shamir-connect/src/server/rate_limit.rs:350](../../../../../crates/shamir-connect/src/server/rate_limit.rs#L350); [crates/shamir-connect/src/server/rate_limit.rs:361](../../../../../crates/shamir-connect/src/server/rate_limit.rs#L361).

<a id="review-12-1"></a>

### Claim 12.1 — Vec::with_capacity constant off by 2 in AuthMessage::build

Status: `confirmed-open`. Current risk: `nit`.

Under-allocation remains, but the review's arithmetic is wrong: AUTH_V1 is 13 bytes, making the fixed part 143, not 144. Allocation growth is possible; a guaranteed particular allocator action was not measured.

Evidence: [crates/shamir-connect/src/common/domain_tags.rs:17](../../../../../crates/shamir-connect/src/common/domain_tags.rs#L17); [crates/shamir-connect/src/common/auth_message.rs:82](../../../../../crates/shamir-connect/src/common/auth_message.rs#L82); [crates/shamir-connect/src/common/auth_message.rs:97](../../../../../crates/shamir-connect/src/common/auth_message.rs#L97).

<a id="review-12-2"></a>

### Claim 12.2 — Module doc points at nonexistent test-vectors/auth_v1/

Status: `confirmed-open`. Current risk: `nit`.

The stale directory reference remains; vectors are stored directly under test-vectors.

Evidence: [crates/shamir-connect/src/common/auth_message.rs:6](../../../../../crates/shamir-connect/src/common/auth_message.rs#L6); [crates/shamir-connect/test-vectors/README.md:11](../../../../../crates/shamir-connect/test-vectors/README.md#L11).

Grouping/duplicate: `style-claude-md.md#5`. This row is not another independent defect.

<a id="review-12-3"></a>

### Claim 12.3 — Unused unicode-normalization dependency

Status: `confirmed-open`. Current risk: `nit`.

The direct dependency remains while normalization code calls precis-profiles and never references unicode_normalization. Exact upstream NFC behavior was not independently inspected.

Evidence: [crates/shamir-connect/Cargo.toml:40](../../../../../crates/shamir-connect/Cargo.toml#L40); [crates/shamir-connect/src/common/username.rs:26](../../../../../crates/shamir-connect/src/common/username.rs#L26); [crates/shamir-connect/src/common/username.rs:46](../../../../../crates/shamir-connect/src/common/username.rs#L46).

<a id="review-12-4"></a>

### Claim 12.4 — validate_client_kdf_safe returns Result<(), String>

Status: `confirmed-open`. Current risk: `nit`.

The public String error remains. Its diagnostic is discarded by handshake; current stricter validation runs first, so the outer-cap rejection there is presently unreachable.

Evidence: [crates/shamir-connect/src/common/kdf_params.rs:77](../../../../../crates/shamir-connect/src/common/kdf_params.rs#L77); [crates/shamir-connect/src/client/handshake.rs:209](../../../../../crates/shamir-connect/src/client/handshake.rs#L209).

Grouping/duplicate: `api-wire-protocol.md#8`. This row is not another independent defect.

<a id="review-12-5"></a>

### Claim 12.5 — Library expect panics for OS RNG failure and pre-epoch clock

Status: `confirmed-open`. Current risk: `low`.

Environment failures still panic. The clock panic is documented; these are not input-controlled programmer-invariant assertions.

Evidence: [crates/shamir-connect/src/common/crypto.rs:69](../../../../../crates/shamir-connect/src/common/crypto.rs#L69); [crates/shamir-connect/src/common/crypto.rs:219](../../../../../crates/shamir-connect/src/common/crypto.rs#L219); [crates/shamir-connect/src/common/time.rs:21](../../../../../crates/shamir-connect/src/common/time.rs#L21).

Grouping/duplicate: `error-handling-lifecycle.md#6`. This row is not another independent defect.

<a id="review-12-6"></a>

### Claim 12.6 — Dead let _ = constant_time_eq suppressor

Status: `confirmed-open`. Current risk: `nit`.

The statement still merely consumes an unused function import; it performs no comparison.

Evidence: [crates/shamir-connect/src/server/handshake.rs:232](../../../../../crates/shamir-connect/src/server/handshake.rs#L232).

Grouping/duplicate: `style-claude-md.md#4`. This row is not another independent defect.

<a id="review-12-7"></a>

### Claim 12.7 — Ticket ciphertext length silently truncates at u16

Status: `confirmed-open`. Current risk: `low`.

The public serializer casts an unrestricted ciphertext Vec length and then writes all bytes. from_bytes rejects the resulting mismatch. Ordinary issued tickets are small, but oversized public TicketWire values are constructible.

Evidence: [crates/shamir-connect/src/server/ticket.rs:106](../../../../../crates/shamir-connect/src/server/ticket.rs#L106); [crates/shamir-connect/src/server/ticket.rs:118](../../../../../crates/shamir-connect/src/server/ticket.rs#L118); [crates/shamir-connect/src/server/ticket.rs:134](../../../../../crates/shamir-connect/src/server/ticket.rs#L134).

<a id="review-12-8"></a>

### Claim 12.8 — Undocumented rotation threshold asymmetry

Status: `confirmed-open`. Current risk: `nit`.

Broadcast verification requires over 60 seconds remaining; recovery requires only a future deadline. The recovery bound is documented, but the rationale for the difference is not. No functional defect is established.

Evidence: [crates/shamir-connect/src/client/rotation.rs:63](../../../../../crates/shamir-connect/src/client/rotation.rs#L63); [crates/shamir-connect/src/client/rotation.rs:89](../../../../../crates/shamir-connect/src/client/rotation.rs#L89); [crates/shamir-connect/src/client/rotation.rs:135](../../../../../crates/shamir-connect/src/client/rotation.rs#L135).

## Corrections and qualified non-findings

- Do not characterize early callback execution as a demonstrated persistent MITM in the shipped Rust client. Its callback uses local capture and returns on verification failure; correctly bound TLS also prevents the described SCRAM relay.
- AUTH_V1 is 13 bytes. The current fixed layout is 143 bytes and alice produces 148 bytes, contradicting the asserted 149-byte length.
- TTL-expiration coverage exists at integration_changepw.rs:195; only exact boundaries and regression coverage are missing.
- The suggested stale-checkpoint fix should authenticate the checkpoint's corresponding log entry, not simply ignore checkpoint_hmac whenever a newer tail exists.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-connect -- Correctness & TDD-coverage

## Summary

The crate is largely well-tested — an unusually strong pinned test-vector suite (`test-vectors/`), concurrency regression tests with `Barrier`/`spawn_blocking` designs, and boundary tests for §7.5/LRU/refill semantics. Against CLAUDE.md's Red/Green/Refactor discipline, however, several real logic bugs survive in security-sensitive orderings and edge cases precisely where the suite has gaps: a TOFU pin callback that fires before identity verification completes, the #1090 refill-watermark hazard still present in the sibling `rate_limit.rs` site it was fixed for, an audit-chain truncation verifier that false-positives on the documented steady state, one broken declared feature combination, and two literally vacuous assertions. Findings below are ranked most severe first.

## Findings

### 1. TOFU pin callback fires before Ed25519 identity verification completes
- **File:line:** `crates/shamir-connect/src/client/handshake.rs:264-294` (`ClientHandshake::process_auth_ok`)
- **Severity:** high
- **Issue:** Step (2) (pin check) invokes `pin_callback(&received_hash)` for the TOFU path *before* step (3) runs `verify_identity`. The doc contract says "pin_callback is invoked exactly when this is the first connection to this host (TOFU): caller decides whether to persist the pin" — and `HandshakeSuccess` carries no pin hash, so persisting inside the callback is the *only* way a caller can implement TOFU. If any later check fails, `process_auth_ok` returns `Err` ("caller MUST disconnect") but the callback has already handed out the (possibly hostile) key hash.
- **Failure scenario:** Active MITM on first connect proxies a real SCRAM exchange to the legitimate server (so `server_signature` verifies), substitutes its own `server_pub_key` in `auth_ok`; the callback fires with SHA256(attacker pub) and a naive caller persists it; the Ed25519 strict verify then fails and the handshake errors — but the attacker's key is now pinned. On every subsequent connection the attacker (who owns that keypair and can sign `identity_input`) passes checks (2) and (3) while proxying SCRAM: persistent MITM. Note the existing tests cover only TOFU-with-valid-sig (`tofu_first_connect_invokes_pin_callback`) or tampered-sig-with-pin (`tampered_identity_sig_aborts_client`) — the TOFU + failed-verify combination, i.e. the ordering hazard itself, is untested.
- **Suggested fix:** Perform the identity-sig verification *before* the TOFU branch (or defer the callback to immediately before `Ok(HandshakeSuccess)`), so the pin is offered only on fully-verified handshakes. Add the red test: TOFU handshake with tampered `identity_sig` must return `Err(ServerSignatureInvalid)` **and** must not have invoked `pin_callback`.

### 2. Rate-limiter refill watermark can regress — the exact #1090 hazard, unfixed at the sibling site
- **File:line:** `crates/shamir-connect/src/server/rate_limit.rs:337-356` (`InMemoryRateLimiter::check`, line 342 `b.last_refill_at_ns = now_ns;`)
- **Severity:** medium
- **Issue:** Same-key callers serialize on the DashMap entry write-lock, but each caller's `now_ns` was captured *before* reaching the map. A thread preempted between reading the clock and acquiring the entry stores an older watermark over a newer one (`b.last_refill_at_ns = now_ns` is unconditional), so the next `check` computes `elapsed` against a regressed point and re-credits an already-credited wall-clock interval. This is precisely the hazard the project fixed in `Session::PostAuthBucket` via `fetch_max` (#1090, 2026-08-11) and documented at length as "unbounded over-refill" in `session.rs` — but the per-subnet `BucketState` site kept the plain store.
- **Failure scenario:** A burst of concurrent `auth_init` from one subnet (the exact scenario the limiter exists for) queues threads on the shard lock; the one holding the oldest `now_ns` commits last, regressing the watermark by its queue time; subsequent checks grant `rate × regression` free tokens. TDD gap: the regression test `out_of_order_now_ns_credits_no_extra_tokens` exists only for `check_post_auth_rate_limit`; no equivalent pins this site.
- **Suggested fix:** Keep the monotonic invariant here too: `b.last_refill_at_ns = b.last_refill_at_ns.max(now_ns)` (compute `elapsed` against the pre-max value), and port the out-of-order-`now_ns` test to `rate_limit_tests.rs`.

### 3. `verify_against_checkpoint` flags a legitimately stale checkpoint as truncation
- **File:line:** `crates/shamir-connect/src/server/audit_chain.rs:267-292`
- **Severity:** medium
- **Issue:** The check rejects unless `checkpoint_seq ∈ {last_seq, last_seq + 1}`. But checkpoints are persisted only periodically (every 60 s / 1000 events per the module's own docs and `AuditChainWriter::append`), so at restart the loaded log almost always contains entries appended *after* the last checkpoint — i.e. `checkpoint_seq < last_seq` is the healthy steady state, not truncation. The doc defines truncation only as "checkpoint is ahead of the chain", yet the code also fires when the checkpoint is *behind* by more than one entry.
- **Failure scenario:** Checkpoint written at `next_seq = 101`; 50 more entries (101–150) appended; crash. On restart, `verify_against_checkpoint(log, 101, …)` returns `Err(TruncationDetected { checkpoint_seq: 101, final_seq: 150 })` although nothing was truncated — a startup false alarm. Tests (`truncation_defence_*`, and `shamir-server/tests/audit_appender.rs`) only exercise exactly-aligned checkpoints, so the defect is invisible to the suite. (Currently no production call site wires this verifier — dormant, but wrong as specified.)
- **Suggested fix:** Truncation ⇔ `checkpoint_seq > last_seq + 1`; additionally require `last_hmac == checkpoint_hmac` only when `checkpoint_seq == last_seq + 1`. Add a red test with a stale checkpoint (`checkpoint_seq << last_seq`) expecting `Ok`.

### 4. `client` feature does not imply `server`, yet client code imports `crate::server::*` unconditionally
- **File:line:** `crates/shamir-connect/Cargo.toml:17-19`; `src/client/handshake.rs:27`; `src/client/bootstrap.rs:17-19`; `src/client/changepw.rs:11`; `src/client/rotation.rs:20`
- **Severity:** medium
- **Issue:** `client = []` and `server = [...]` are declared as independent features, but every client module has an ungated `use crate::server::…` (e.g. `RotationInProgressPayload`, `BootstrapChallenge`, `ChangePwRequest`). With `--no-default-features --features client` the `server` module is cfg'd out and the crate fails to compile (E0433). Default builds mask this; the declared configuration matrix is broken.
- **Failure scenario:** `cargo check -p shamir-connect --no-default-features --features client` — a supported combination per the manifest — errors; any future CI job or downstream trying a client-only build breaks.
- **Suggested fix:** Declare the dependency: `client = ["server"]` (simplest, matches reality), or feature-gate the rotation/bootstrap extensions out of the client surface.

### 5. `dispatch_request` silently omits the #608 post-auth rate-limit gate its "functionally identical" twin has
- **File:line:** `crates/shamir-connect/src/server/dispatch.rs:69-110` (no gate) vs `:150-160` (`dispatch_request_view` calls `session.check_post_auth_rate_limit`)
- **Severity:** medium
- **Issue:** `dispatch_request_view`'s doc says it is "Functionally identical: same §7.5 validity check, same handler dispatch, same outcome shape", and its rate-limit comment claims a "single choke point covering every transport that routes through this function". The owning `dispatch_request` — separately `pub use`-exported from `server/mod.rs` and used by `integration_session.rs` and `benches/hot_paths.rs` — performs lookup + §7.5 check but **no** per-session rate limiting. Two entry points with identical-looking contracts but divergent security behavior is an invariant violation; the divergence is also untested (no test asserts rate limiting on either entry point via `dispatch_request`).
- **Failure scenario:** Any transport or embedder that routes through `dispatch_request` (the non-`_view` variant) gets an unthrottled per-session request path; a compromised/looping authenticated client is bounded only by transport-level limits.
- **Suggested fix:** Add the `check_post_auth_rate_limit` gate to `dispatch_request` (dedupe the common prefix into one helper), or remove `dispatch_request` from the public re-exports and mark it test/bench-only with a doc warning.

### 6. `ServerIdentityState::rotate` / `try_finalize` are load-clone-store races despite the "Atomic:" doc
- **File:line:** `crates/shamir-connect/src/server/rotation.rs:151-180` (`rotate`), `:184-199` (`try_finalize`)
- **Severity:** low
- **Issue:** Both do `let current = (**self.inner.load()).clone(); … self.inner.store(Arc::new(new_inner));` — a non-atomic read-modify-write, while `rotate`'s doc claims "Atomic: previous = current; current = new; …". `ArcSwap` makes each individual load/store atomic, not the sequence.
- **Failure scenario:** (a) Two concurrent `rotate()` calls both pass the overlap pre-check and both store; the second silently orphans the first `RotationOutcome` whose new keypair is no longer installed — clients that pinned it hit a hard `ServerIdentityChanged` on next connect. (b) `try_finalize` (background GC) loading a pre-rotation snapshot can store `previous: None, rotation_until: None` with the *old* keypair after `rotate` committed, erasing the rotation while `current_version_atomic` keeps the incremented version (mirror/inner divergence). Rarity of concurrent admin-op + finalize caps the severity.
- **Suggested fix:** Use an ArcSwap CAS loop (`compare_and_swap`/`rcu`) or serialize both mutators through a small mutex, and re-check the overlap precondition inside the committed state; keep the atomic mirror and inner version from the same committed snapshot.

### 7. Password buffers are not zeroized on early-error returns, violating the documented contract
- **File:line:** `crates/shamir-connect/src/client/handshake.rs:202-238` (zeroize only at line 232, after four `?`-early-exits); `src/client/bootstrap.rs:93-97`; `src/client/changepw.rs:43-72`
- **Severity:** low
- **Issue:** All three functions document the password as "zeroized on return / after use", but every validation failure (`validate_client_limits()?`, `KdfParamsRejected` from `validate_client_kdf_safe`, all-zero server_nonce, `AuthMessage::build`/`DerivedKeys::derive` errors) returns before `password.zeroize()` runs. A hostile or misconfigured server sending an over-limit `kdf_params` challenge — the exact path `validate_client_kdf_safe` exists for — leaves the password unzeroized in the caller's buffer after the handshake aborts.
- **Suggested fix:** Zeroize on all exits: a small drop-guard over the caller's slice, or explicit `password.zeroize()` before each early return (and a test asserting the buffer is zeroed on the `KdfParamsRejected` path).

### 8. changePassword TTL check underflows on clock regression; TTL boundary untested
- **File:line:** `crates/shamir-connect/src/server/changepw.rs:141` (`now_ns - pending.issued_at_ns > CHANGEPW_CHALLENGE_TTL_NS`)
- **Severity:** low
- **Issue:** Plain u64 subtraction. If the wall clock steps backwards (NTP correction) between `start_change_password_challenge` and verification, `now_ns < issued_at_ns` underflows: debug builds panic; release wraps to a huge value and rejects — fail-closed only by accident of two's-complement wrap. Everywhere else in the crate the same pattern uses `saturating_sub` (rate limiters, lockout). Coverage gap: `CHANGEPW_CHALLENGE_TTL_NS` is referenced only in `src/`; no test exercises TTL−1 (accept), TTL+1 (reject), or a backwards clock.
- **Suggested fix:** `match now_ns.checked_sub(pending.issued_at_ns) { Some(elapsed) if elapsed <= TTL => …, _ => Err(AuthFailed) }` (explicitly rejecting future-dated challenges), plus the three boundary tests.

### 9. `encode_details_canonical` is a placeholder that encodes nothing, with an unusable signature
- **File:line:** `crates/shamir-connect/src/server/audit_chain.rs:355-361`
- **Severity:** low
- **Issue:** The function is `pub` and documented as the default encoder for the HMAC-covered `details_canonical_msgpack`, but it takes `&BTreeMap<String, rmp_serde::config::DefaultConfig>` (a msgpack *serializer config* type as the map's value type — meaningless), ignores its input (`let _ = map;`), and returns `Vec::new()`. Any caller wiring audit details through it silently chains empty details into the audit HMAC.
- **Suggested fix:** Implement against a real value type (e.g. serialize a sorted map of `String → rmpv::Value`-equivalent bytes the caller supplies), or delete the function until a real implementation lands — a stub with this signature is a trap in a public API.

### 10. Vacuous variant assertions in the all-zero nonce tests
- **File:line:** `crates/shamir-connect/src/common/tests/auth_message_tests.rs:151` and `:175`
- **Severity:** low
- **Issue:** `matches!(err, crate::common::Error::InvalidInput(_));` — the `matches!` result is evaluated and **discarded** (statement, not `assert!`). The two tests (`rejects_all_zero_client_nonce`, `rejects_all_zero_server_nonce`) only assert "an error occurred" (via `unwrap_err`); the variant check is dead code. A refactor that starts returning e.g. `Error::InvalidUsername` for these paths would pass the suite — a direct violation of the Red/Green discipline (the assertion that would catch the regression doesn't assert).
- **Suggested fix:** `assert!(matches!(err, Error::InvalidInput(_)));` on both lines (grep confirms these are the only two occurrences in the crate).

### 11. `InMemoryRateLimiter::with_rate(_, 0)` breaks the limiter after warmup
- **File:line:** `crates/shamir-connect/src/server/rate_limit.rs:186-192, 329-367`
- **Severity:** low
- **Issue:** `rate_per_sec = 0` passes through unvalidated. During warmup `effective_rate_per_sec` clamps to ≥ 1, but after warmup it returns 0: `capacity_at_rate(0) == 0`, so the first request's `or_insert_with` computes `capacity - cost = 0 − 1e9` (u64 underflow — debug panic, release wraps to ~u64::MAX, i.e. a silently unlimited bucket), and the throttle branch divides by `rate as u64` (division by zero).
- **Suggested fix:** Validate `rate_per_sec >= 1` in `with_rate`/`with_snapshot_sink_and_rate` (return or clamp), or make `effective_rate_per_sec` `.max(1)` unconditionally.

### 12. Nits
- **File:line:** `src/common/auth_message.rs:82` — **nit** — `Vec::with_capacity(142 + username.len())`: the fixed fields sum to 144 (14+2+32+32+16+4+4+4+1+1+1+32+1), so every `build` does one guaranteed realloc; the module doc's "149 total for a 5-byte username" is consistent with 144, so only the constant is off by 2. Fix the constant (or compute it once as a `const`).
- **File:line:** `src/common/auth_message.rs:6` — **nit** — module doc points at `test-vectors/auth_v1/`; vectors actually live flat in `test-vectors/`.
- **File:line:** `Cargo.toml:40` — **nit** — `unicode-normalization` dependency is unused in crate code (NFC is applied inside `precis-profiles::enforce`); remove it or annotate why it is kept.
- **File:line:** `src/common/kdf_params.rs:77` — **nit** — `validate_client_kdf_safe` returns `std::result::Result<(), String>`, against the workspace rule "thiserror for library error enums" (tests then string-match on the message text). Fold into `Error::KdfParamsRejected`/`Error::Crypto` or a small thiserror enum.
- **File:line:** `src/common/crypto.rs:66-70, 215-222`; `src/common/time.rs:18-23` — **nit** — `.expect(...)` panics in library code (OsRng failure, pre-epoch clock). Documented, but per CLAUDE.md error handling these should be `Result` at the library boundary.
- **File:line:** `src/server/handshake.rs:232` — **nit** — `let _ = constant_time_eq;` is a dead statement whose only purpose is suppressing an unused-import warning; remove the import instead.
- **File:line:** `src/server/ticket.rs:118` — **nit** — `(self.ciphertext.len() as u16)` silently truncates ciphertexts > 64 KiB; `from_bytes` would then reject the corrupt ticket. Unreachable with current ~150-byte plaintexts, but a `debug_assert!`/error beats silent truncation.
- **File:line:** `src/client/rotation.rs:63` vs `:135` — **nit** — the broadcast-event path requires `transition_until_ns > now + 60 s` while the orphan-recovery path requires only `> now`. Probably intentional (event-vs-auth_ok freshness), but the asymmetry is undocumented.

</details>

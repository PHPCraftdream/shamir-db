# TOKEN-T2a core design

`TOKEN_API.md` v1 sections 2, 3, 5, 6, 7 and 10 are normative; this implementation remains transport/storage/runtime-free. Token format uses canonical URL_SAFE_NO_PAD decode into zeroizing fixed arrays, and output is built in one preallocated `Zeroizing<String>` via `encode_string`. Generated entropy destinations are zeroizing before OS fill. MAC is exactly `domain || format_version || key_id || id || secret`; comparison uses `subtle`. Records use fixed byte arrays with `serde_bytes`, named MessagePack, deny-unknown-fields, and structural validation.

## Verification and owner-floor contract

Order is parse → lookup (miss computes exactly one dummy HMAC) → stored key/version check → constant-time MAC → revoked → expired (`now >= expires`) → owner known and strict `created_at_ns > floor` → channel → success. `OwnerFloors` is object-safe and synchronous; `None` means unknown/invalid owner. Format version is fixed at v1 in the verifier; `VerifyCtx` intentionally has no format-version field per the exact brief API. `VerifyCtx` Debug redacts its key.

## Revocation publication

TokenState stores immutable record data and an atomic revocation timestamp (`0` live). A first revoke publishes using CAS Release; readers load Acquire. A reader that observes the CAS synchronizes with it. Across threads, post-return observation requires caller synchronization (same-thread sequencing, join, barrier, or channel); bare wall-clock assertion is insufficient. `Some(0)` in a loaded record is normalized to sentinel timestamp `1`, never treated as live. A TokenOrigin retains the state Arc and checks Acquire revocation plus expiry.

## Mirror bounded serialization

The mirror uses 64 fixed Fx-hashed atomic gate stripes, avoiding a per-ID registry whose attacker-influenced key cardinality could grow forever. IDs mapping to a stripe serialize; unrelated stripes proceed independently. A local RAII guard always releases the stripe, including unwind. Index mutations acquire the stripe before touching either map, hold only one DashMap guard at a time, and preserve owner-index consistency. Stripe collisions affect throughput only, not correctness; no lock identity is reclaimed or replaced.

## Token session accounting and reclamation

Each token/owner counter entry stores an atomic active count and atomic lease count. Acquisition increments leases while holding the map's exclusive entry guard; this prevents pruning/replacement during lookup-to-use. CAS reserves token then owner capacity, rolling back token reservation on owner-cap failure; duplicate SID insertion rolls back both. Leases protect counter cells during admission and removal operations; after admission releases its leases, a positive published-session count protects the cells from pruning until removal. Removal acquires temporary leases before decrementing exactly once. A lease release first decrements the lease count, then attempts pruning under the same map's exclusive entry guard; it checks cell identity and zero leases/count while acquisition is excluded. Thus no naked strong-count check or lookup-to-counter TOCTOU exists, and a new cell cannot replace one while any active operation or live session depends on it. Failed zero-cap attempts also release/prune leases. SCRAM SessionStore remains independent.

## Tests and counterfactual regressions

`format_tests`: roundtrip/canonical base64url, malformed separators/alphabet, non-ASCII and invalid UTF-8, huge input, both tail-bit positions, every TokenId final byte, and Debug redaction.
`mac_tests`: independently assembled layout, component perturbations, and every tag-byte mismatch.
`record_tests`: MessagePack roundtrip, unknown field, shape limits and channel matrix.
`mirror_tests`: duplicate preservation, owner-index cleanup under races, idempotent revocation with held-state reader, zero-timestamp normalization, and `deterministic_colliding_stripes_serialize_remove_and_insert` (added and read statically; NOT RUN CI).
`verify_tests`: decision boundaries and HMAC work count for unknown and wrong-secret.
`token_session_tests`: cap rejection/acquisition counts, rollback on owner cap and duplicate SID, zero-cap behavior, re-admission after removal, get/fan-out consistency, barrier-started concurrent admissions with exactly the successful SIDs observed, SCRAM-store independence, and counter pruning/reuse.

A targeted retention regression test is present: `counter_cells_are_reclaimed_after_rejection_removal_and_churn` checks the test-only `counter_cells_hint` helper to catch retained cells. It was added and read statically; execution is NOT RUN CI.

## Continuation review (2026-11-01)

This continuation supersedes stale statements above where they conflict with the current source. The mirror uses exactly 64 fixed Fx-hashed atomic gate stripes, not a retained per-ID gate registry (`crates/shamir-connect/src/server/auth/token/mirror.rs:11,75-80,101-115`). Mirror operations acquire the stripe before index mutations and RAII releases it. The older comments in `mirror.rs:19-21,69-71` describing retained per-ID gate cells and unbounded growth are historical; the correction immediately following them is the current design. Revocation's Release CAS / Acquire load publication does not make a bare wall-clock post-return assertion valid across threads: observation after `revoke` returns requires a synchronization edge, e.g. same-thread sequencing, join, barrier, or channel (`mirror.rs:12-18,45-57`).

Session counter leases are acquired through the map's exclusive entry and increment `leases` before dropping that entry guard (`crates/shamir-connect/src/server/state/token_session.rs:38-50`). On last-lease release, cleanup reacquires exclusive entry access and removes only if the entry points to the same cell (`Arc::ptr_eq`) and both `leases == 0` and `count == 0` (`token_session.rs:52-64`). This protocol prevents identity splitting; the old lifetime-retained-cell comment at lines 26-27 is historical. A positive published-session count protects a cell from pruning. Admission reserves token then owner capacity with CAS; owner-cap rejection rolls back token, duplicate SID rolls back both, and insertion occurs only for a vacant SID (`token_session.rs:85-99,111-130`). Removal takes temporary leases, removes the session once, and decrements both counts exactly once (`token_session.rs:132-145`).

## Disputed specification/dependency point

`TOKEN_API.md:58` specifies an `scc` map for in-memory token state. The task brief explicitly requires DashMap and forbids adding dependencies; the implementation uses DashMap (`mirror.rs:3,76-80`; `token_session.rs:4,66-70`). This conflict is **DISPUTED**, not silently resolved: implementation follows the brief. No dependency change is authorized here.

## Review and verification status

No current-session local compilation was done; any inherited statement that a check is “in progress” is historical only. Per operator, all check/clippy/tests are **NOT RUN** in this task and are for CI. Do not infer passing tests or full implementation completion; orchestrator review remains outstanding.

Corrected test-review notes: format/verify test descriptions and expectations must not contradict the actual brief or implementation; treat prior coverage wording as review guidance, not evidence of execution. Retain deterministic concurrency controls (barriers/channels and explicit synchronization for post-return revocation observations), and include additional record coverage where required by the brief. No test results are asserted here.

# P1 stage G10 — p1-interner-quota

## Goal

Per-repo interner quota, alias decode guard, hasher threat model. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row G10): Per-repo interner quota (1,048,576 names / 64 MiB) enforced in touch_ind with code interner_quota_exceeded, replication/seed/recovery counted but exempt; alias-keyed request collections capped at 1000 entries at decode; hasher threat model documented (ruling RU8).

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-TYPES-2EB39DFD — Append-only interner accepts unlimited untrusted key names; FxHash rationale inverted

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-db/src/shamir_db/execute/admin_meta/admin_interner.rs:129; crates/shamir-types/src/core/interner/interner.rs:149; changed by: none
- R0 note (includes the fix site): OBSERVED: Store Write still authorizes arbitrary-name InternerTouch followed by append-only persistence, and the allocator has no cumulative name or byte admission quota. Per-input allocation caps do not bound growth across repeated accepted requests; a concrete FxHash collision attack remains unestablished. Fix site: crates/shamir-types/src/core/interner/interner.rs needs cumulative admission accounting covering every insertion path.

### RV-COLLECTIONS-DA7D2CD3 — Unseeded FxHasher exported as THE workspace hasher is fed client-controlled string keys downstream — precomputable HashDoS amplifier

- R0 verdict: `unverified`, risk `medium`; evidence: crates/shamir-query-types/src/ops/batch/req/batch_request.rs:125; crates/shamir-connect/src/server/infra/dispatch.rs:164; changed by: none
- R0 note (includes the fix site): OBSERVED: client-controlled aliases still deserialize into deterministic Fx-backed TMap after session/rate admission. Missing premise: a practical colliding UTF-8 key family demonstrating amplification within current input and query limits. Fix site: shamir-query-types/src/ops/batch/req/batch_request.rs is the alias-admission seam for bounded or adversarially resistant decoding if that premise is established.

## Binding rulings

These decisions are final; do not re-litigate them.

### Binding ruling RU8 — interner quota for untrusted names and hasher policy (RV-TYPES-2EB39DFD, RV-COLLECTIONS-DA7D2CD3)

Decided by 2x rush reviewer + @oh. All three agree on a per-repository, append-only quota enforced in the shared interner (not only in the `InternerTouch` handler) with typed error code `interner_quota_exceeded`. They disagreed on the hasher: the two reviewers wanted seeded hashers on the peer-name maps, @oh showed the main peer-keyed surface is `QueryValue::Map` itself (Fx `TMap` inside the core value type), so seeding individual maps does not hang together and nothing demonstrates a collision family. **Final: keep Fx, add a cheap decode-time guard, document the threat model; numbers and exemptions as below (@oh's variant).**

RULING: per-repo quota of 1,048,576 distinct names and 64 MiB of total name bytes (UTF-8 `str::len()`), overridable by env/tunables, enforced inside `Interner::touch_ind` on the new-name path; replication, seed and recovery are counted but never rejected; Fx stays everywhere; alias-keyed request collections get a decode-time entry limit of 1000; CLAUDE.md pillar 4 wording is flagged for the OWNER (do not edit CLAUDE.md).

Acceptance:
1. shamir-tunables `instance_defaults` gains `INTERNER_MAX_NAMES: u64 = 1_048_576` and `INTERNER_MAX_NAME_BYTES: u64 = 64 * 1024 * 1024`, env names `SHAMIR_INTERNER_MAX_NAMES` / `SHAMIR_INTERNER_MAX_NAME_BYTES`, pure parse functions in the style of the existing ones (unparsable or 0 falls back to the default; there is no unlimited mode). Unit tests for the parse policy.
2. shamir-types gets `InternerQuota { max_names, max_bytes }` plus two `AtomicU64` counters (names, sum of UTF-8 lengths; never `len()` of a scc map). `Interner::new()` has no quota (scratch interners unaffected); the repo's interner manager attaches the quota at load with counters seeded from the hydrated entries.
3. In `touch_ind`: check the quota only after the fast-path miss; reserve with CAS (names first, then bytes, refund names if bytes fail) BEFORE `current_id.fetch_add`, so a rejected touch consumes no id and does not change `generation()`; refund the reservation if the forward-map insert loses its race. Test: N threads touching distinct names with cap K produce exactly K `New` results and the counters equal K.
4. `touch_ind` returns a typed `thiserror` error `InternerError::QuotaExceeded { kind: Names|Bytes, limit }` instead of `&'static str`. Re-touching an existing name after exhaustion is still `Ok(Exists)`. Exact-boundary tests for both limits.
5. Exempt paths use `touch_ind_unmetered` (counts, never rejects): replication apply, table-manager replication, seed writer, seed/schema install, recovery replay, hydration; `touch_with_id` and `with_state` count but never reject. Every other caller stays metered. Tests: replication apply, seed install, recovery replay and hydration succeed above the cap and the next metered `touch_ind` is then rejected.
6. New `DbError::InternerQuotaExceeded { kind, limit }` maps to wire code `"interner_quota_exceeded"` (via `DbError::code()`); the message names the repo, the kind, the limit and the env var; non-retryable. Implicit writes and tx commits that hit it leave no WAL record and no visible row. Tests: a new-field insert, a tx batch and an `InternerTouch` each over the cap. If shamir-client-ts has a closed list of error codes, add it.
7. `InternerTouch` on quota failure still calls `persist()` for names already interned in that request, then returns the error with no mappings; test: reopen and confirm the prefix is present.
8. No panics: remove `expect` on `touch_ind` in the layered interner and in schema_codec and propagate the error; `pre_intern_select_keys` returns the error instead of `let _ =`; test: a HAVING on a new alias at the cap returns the code. No non-test, non-bench `touch_ind(..).expect/unwrap` may remain under `crates/*/src`.
9. At load, warn when a repo is at or above 80% of either limit.
10. Decode guard: `BatchRequest.queries` (including nested sub-batch maps), `interner_epochs` and `return_only` are capped at `BatchLimits::CEILING.max_queries` (1000) during deserialization (the visitor stops at entry 1001). Tests: 1000 entries decode; 1001 give `invalid_request` for each field.
11. The `THasher` docs (shamir-collections) record: which peer-keyed Fx surfaces exist; the bounds that apply (frame size, rate limit, decode guard, interner quota); the reopen trigger: a demonstrated colliding UTF-8 family means swapping `THasher` globally, measured with the bench harness, not seeding individual maps.
12. RV-TYPES-2EB39DFD is closed by items 1-9; RV-COLLECTIONS-DA7D2CD3 is closed as an accepted residual by items 10-11. CLAUDE.md is NOT edited (pillar 4 says "we don't accept untrusted hash inputs here", which is false in fact: flag for the owner).

Compat: disk none (counters rebuilt from hydrated entries, never persisted); wire: one new error code; requests with more than 1000 aliases were already rejected, their code changes from `limits` to `invalid_request`; no protocol version bump; repos already above the cap after upgrade still load and stay readable, only new names are refused until the operator raises the knob.


## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.

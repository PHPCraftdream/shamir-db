# P1 stage G2 — p1-connect-audit

## Goal

Audit chain HMAC formula, checkpoint, persistence errors. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row G2): The audit HMAC matches its published formula (or the formula is corrected with a migration); a stale checkpoint is not reported as truncation; appender persistence failures reach the writer; the placeholder canonical encoder is removed or implemented; the chain mutex is documented and minimal.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-CONNECT-7DBC7BDB — Audit HMAC implementation omits the extra prev_hmac prefix required by its published formula

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/server/infra/audit_chain.rs:122; docs/guide-docs/client-server-protocol-spec/IMPLEMENTATION_GUIDE.md:349; changed by: none
- R0 note (includes the fix site): OBSERVED: compute_entry_hmac still authenticates canonical bytes alone, while the normative formula additionally prefixes prev_hmac; canonical bytes already include prev_hmac at their end. Fix site: server/infra/audit_chain.rs needs an explicit durable-format/migration decision before reconciling the helper with the published formula.

### RV-CONNECT-48F29207 — verify_against_checkpoint flags a legitimately stale checkpoint as truncation

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/server/infra/audit_chain.rs:281; crates/shamir-connect/src/server/infra/audit_chain.rs:291; changed by: none
- R0 note (includes the fix site): OBSERVED: verification still requires checkpoint sequence to equal the final sequence or its successor, so a checkpoint with next_seq 101 rejects a valid extension through 150; the retention change did not alter this guard. Fix site: server/infra/audit_chain.rs must authenticate the checkpoint's position within an extended chain rather than require tail alignment.

### RV-CONNECT-7FFA5398 — AuditAppender persistence failures are unreportable to AuditChainWriter

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-connect/src/server/infra/audit_chain.rs:355; crates/shamir-server/src/security/observability/logging/audit_appender.rs:706; changed by: none
- R0 note (includes the fix site): OBSERVED: appender methods still return unit after chain advancement, and production checkpoint still attempts checkpoint persistence after logging flush failure; the runtime-aware flusher change does not alter this ordering. Fix site: server/infra/audit_chain.rs needs reportable persistence outcomes coordinated with failure-gated checkpointing in audit_appender.rs.

### RV-CONNECT-A2618127 — encode_details_canonical is a broken placeholder public API

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-connect/src/server/infra/audit_chain.rs:369; changed by: none
- R0 note (includes the fix site): OBSERVED: the public helper still takes serializer configurations, ignores its map and returns an empty Vec; searches found no caller that changes this API-level trigger. Fix site: server/infra/audit_chain.rs must replace or withdraw the placeholder encode_details_canonical contract.

### RV-CONNECT-D068976D — AuditChain::inner: undocumented mutex containing HMAC, allocations, and an entry clone

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-connect/src/server/infra/audit_chain.rs:204; crates/shamir-connect/src/server/infra/audit_chain.rs:227; changed by: none
- R0 note (includes the fix site): OBSERVED: bounded retention does not move string conversion, canonical-byte allocation, HMAC or entry cloning outside the mutex, and no contention-model justification was added. Fix site: crates/shamir-connect/src/server/infra/audit_chain.rs, reduce materialization under the sequencing lock and document its contention model.

## Binding rulings

These decisions are final; do not re-litigate them.

### Binding ruling RU4 — audit chain HMAC formula (RV-CONNECT-7DBC7BDB)

Decided by 2x rush reviewer + @oh (unanimous, 3/3): **option B — the code stays, the specification is corrected and pinned.**

RULING: the authenticated bytes stay `HMAC-SHA256(audit_chain_key, canonical(entry_without_hmac))`. `canonical(...)` already ends with the 32-byte `prev_hmac`, so a `prev_hmac ||` prefix would authenticate the same bytes twice (no security gain). No format version, no dual-formula verification, no migration.

Acceptance:
1. `docs/guide-docs/client-server-protocol-spec/IMPLEMENTATION_GUIDE.md` (formula near lines 347-365) and the module doc of `crates/shamir-connect/src/server/infra/audit_chain.rs` (lines ~9-32) state the formula above, say that `prev_hmac` is bound because it is the final 32 bytes of the canonical bytes, and that implementations MUST NOT add a separate prefix. Also fix the prose copy in `docs/dev-artifacts/research/crate-extraction/05-network-server-client.md:49`. A repo-wide search for `prev_hmac ||` outside `docs/dev-artifacts/audits/**` finds nothing.
2. The spec gains a "Test vectors" subsection with at least two chained entries: V1 (seq 1, zero `prev_hmac`, empty details, zero session prefix) and V2 (seq 2, `prev_hmac` = V1's hmac, non-zero `session_id_prefix`, non-empty details, non-ASCII UTF-8 in `user`). For each: the fixed key (hex), every field value, the full canonical hex, the expected 32-byte HMAC hex. The expected hex is derived INDEPENDENTLY of `canonical_bytes`/`compute_entry_hmac` (for example Python stdlib `hmac`/`hashlib` over a hand-written byte string); record the derivation recipe next to the vectors. The executor cannot run Python against the implementation under test; derive the bytes by hand-written construction from the documented layout and say how.
3. New test in `crates/shamir-connect/src/server/tests/infra_tests/audit_chain_tests.rs` (registered in its manifest): `canonical_bytes(&entry)` equals the full canonical hex literal and `compute_entry_hmac(&key, &entry)` equals the HMAC hex literal, for both vectors (literals, never computed through the code under test); `AuditChain::verify_chain(&key, &[V1, V2])` is `Ok`; flipping one byte of V2's `prev_hmac` yields `ChainBroken { at: 1 }`; `HMAC(key, prev_hmac || canonical)` over V1 differs from V1's expected HMAC (the rejected formula cannot pass silently); wrong key / modified payload are rejected.
4. A fixed, literal two-entry pre-remediation chain fixture is verified, resumed from its checkpoint and extended with one appended entry (unchanged formula), and the combined chain verifies — proving existing logs stay valid.
5. Production HMAC computation, `audit_appender.rs`, the log line format and the checkpoint format stay byte-identical.
6. NEW (same file, in scope): `canonical_bytes` narrows field lengths with `as u8` / `as u16` (`audit_chain.rs` ~lines 103-112), which makes the encoding non-injective for fields longer than 255 bytes (event: 65535). Make oversized fields an explicit error (checked conversion) instead of silent truncation; bytes for in-range fields stay identical; add a test.
7. RV-CONNECT-48F29207 (checkpoint staleness) and RV-CONNECT-7FFA5398 (void appender persistence errors) are separate cards of the same stage; do them as their own commits' worth of change with their own tests, not folded into the formula work.

Compat: none (no wire/disk change). An external verifier that followed the erroneous prefix text must adopt the corrected text; publish the correction as a spec erratum next to the vectors.


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

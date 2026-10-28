# P1 stage H13 — p1-types-interner-view

## Goal

Types: interner reservations/races, record view semantics. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row H13): Published ID reservations cannot stale subscription filters; repeated key IDs have one defined answer; container materialisation is `None` as documented; de-intern errors propagate; publication races and release-mode guard fixed; `touch_ind` signature honest; local oracle tests; per-key String allocation avoided.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-TYPES-056200A9 — Published ID reservations can permanently stale subscription filters

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-types/src/core/interner/interner.rs:157; crates/shamir-server/src/engine/subscriptions/filter/filter_eval.rs:99; changed by: none
- R0 note (includes the fix site): OBSERVED: generation exposes the reservation counter before name publication, and touch_with_id can fill lower-ID holes without changing it; the subscription cache records the generation sampled before compilation and reuses matching generations. No publication guard closes the stated stale-filter schedule. Fix site: crates/shamir-types/src/core/interner/interner.rs must provide a publication-based invalidation generation.

### RV-TYPES-825C92A8 — Repeated storage key IDs produce first-wins and last-wins answers on accepted S-write bytes

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-types/src/record_view/lens/view.rs:117; crates/shamir-engine/src/table/write/insert.rs:354; changed by: none
- R0 note (includes the fix site): OBSERVED: lens lookup still returns the first repeated ID, while index and decoded output maps overwrite with the last; S-write validation checks resolution without duplicate rejection and stages the original bytes. Completed-span indexing changes neither policy. Fix site: crates/shamir-types/src/codecs/interned/validate_keys.rs must enforce a consistent duplicate-key admission policy at the S-write seam.

### RV-TYPES-33ABB83B — `HavingView::materialize_at` returns `Some(Null)` for containers, contradicting its own safety comment ("containers return None")

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-types/src/record_view/record_ref.rs:539; crates/shamir-types/src/record_view/record_ref.rs:581; changed by: none
- R0 note (includes the fix site): OBSERVED: HavingView still wraps conversion in Some, and List, Set, and Map still convert to Null despite the adjacent containers-return-None promise. The changed single-pass implementation belongs to RecordView, not HavingView. Fix site: crates/shamir-types/src/record_view/record_ref.rs must reconcile HavingView container materialization with its contract.

### RV-TYPES-97A84E4F — `RecordRef::to_query_value` swallows de-intern errors into `QueryValue::Null`

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-types/src/record_view/record_ref.rs:223; crates/shamir-engine/src/query/read/projection/select_projection.rs:238; changed by: none
- R0 note (includes the fix site): OBSERVED: Both RecordRef implementations still replace conversion errors with Null, and full-record projection calls that conversion without an error channel; missing reverse IDs still produce codec errors underneath. Fix site: make conversion failure distinguishable from legal Null in crates/shamir-types/src/record_view/record_ref.rs and propagate it through projection.

### RV-TYPES-1EBA1D8A — Transient forward-before-reverse publication window (and rollback window) visible to racing third-party readers

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-types/src/core/interner/interner.rs:169; crates/shamir-types/src/core/interner/interner.rs:404; changed by: none
- R0 note (includes the fix site): OBSERVED: both insertion paths discard the forward insertion guard before reverse publication; touch_with_id can then remove an already exposed forward association on collision. The reverse-write mutex does not protect forward readers across this window. Fix site: crates/shamir-types/src/core/interner/interner.rs must coordinate forward visibility with reverse publication and collision rollback.

### RV-TYPES-3CFA3FB6 — `touch_ind` racing `touch_with_id` on the same id is guarded only by `debug_assert!` — silent in release

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-types/src/core/interner/interner.rs:223; crates/shamir-types/src/core/interner/interner.rs:449; changed by: none
- R0 note (includes the fix site): OBSERVED: touch_with_id publishes its reverse slot before advancing current_id, while touch_ind leaves its forward entry intact when its reverse-slot set loses, checking failure only with debug_assert. The live drainer still calls touch_with_id, so a startup-only exclusion is not an API guard. Fix site: crates/shamir-types/src/core/interner/interner.rs must coordinate explicit-ID publication with automatic allocation and handle collisions in release builds.

### RV-TYPES-5DD242F6 — `Interner::touch_ind` returns a `Result` with no reachable `Err`; `touch_with_id` returns stringly-typed errors

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-types/src/core/interner/interner.rs:138; crates/shamir-types/src/core/interner/interner.rs:348; changed by: none
- R0 note (includes the fix site): OBSERVED: touch_ind still has only Ok branches despite its Result signature, while touch_with_id constructs genuine conflicts as String errors. The inspected drainer propagates those errors rather than interpreting substrings. Fix site: crates/shamir-types/src/core/interner/interner.rs retains both public error-surface issues and requires coordinated API changes.

### RV-TYPES-6C7C9E1E — HavingView container semantics lack a types-local discriminating oracle; external scalar coverage exists

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-types/src/record_view/record_ref.rs:515; crates/shamir-engine/src/query/read/tests/aggregation/numeric_and_functions/qv_aggregate_tests.rs:112; changed by: none
- R0 note (includes the fix site): OBSERVED: types-local source search still finds no HavingView test construction; the external oracle asserts scalar sum_age filtering, not container materialization, nested paths, or unknown keys. RecordView performance tests do not fill this gap. Fix site: crates/shamir-types/src/record_view/tests needs direct HavingView contract coverage.

### RV-TYPES-86E037B2 — Mid-function imports in production code violate imports-at-top

- R0 verdict: `confirmed-open`, risk `nit`; evidence: crates/shamir-types/src/core/interner/interner.rs:161; crates/shamir-types/src/types/ids/record_id.rs:85; changed by: none
- R0 note (includes the fix site): OBSERVED: both function-local Entry imports and the initializer-local SeedableRng import remain, without a documented name-collision or target-validity exception. This is imports-at-top debt, not a runtime defect. Fix sites: crates/shamir-types/src/core/interner/interner.rs and crates/shamir-types/src/types/ids/record_id.rs retain the local imports.

### RV-TYPES-58E7E6DC — Zerocopy msgpack decoder allocates an owned `String` per map KEY even though keys go straight into the interner

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-types/src/codecs/interned/messagepack.rs:130; crates/shamir-types/src/codecs/interned/messagepack.rs:330; changed by: none
- R0 note (includes the fix site): OBSERVED: each map key still passes through read_str returning an owned String and is immediately borrowed for interning; new capacity limits do not remove that copy. This confirms structural allocation cost, not the unsupported WAL-hot-path impact. Fix site: crates/shamir-types/src/codecs/interned/messagepack.rs can borrow validated key text from the input before interning.

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

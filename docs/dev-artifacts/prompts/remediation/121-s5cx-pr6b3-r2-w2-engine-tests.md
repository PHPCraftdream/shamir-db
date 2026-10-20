# S5c-X PR6b-3 / R2-W2 — engine test fixes + acceptance-coverage repairs (review round 2)

Worktree: current directory. R2-W1 (engine production fixes) has landed: single public
`pub use ...FenceRecord` in `meta/repl/bookmark.rs`; `pub mod seed_install;` in `repo/mod.rs`;
`PreparedTable` reduced to `{name, ops}`; `admit_follower` capability gate for BOTH branches;
gate init derives receipt-native visibility from a durable v3 record (and REFUSES gate
initialization on a legacy/malformed bookmark record with
`DbError::FollowerStateRefused`); `classify_follower_bookmark` runs BEFORE WAL replay in both
`from_factory*` open paths; `bookmark_v3::receipt_pointer_resolves` exists.

## Hard rules (verbatim)
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
  command that mutates the working tree or index. Only edit files; the orchestrator commits.
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` or any other compile or test
  command, by you or any child agent. Only `cargo fmt -p shamir-engine` as your LAST step.
- Never silence a lint with `allow`.
- Layout: <=7 entries/folder, <=1000 lines/file. `tx/tests/` is ALREADY over the entry cap —
  add NO new files there; extend existing files. No machine-specific absolute paths.
- Tests in tests/ folders, wired into manifests; JSON literals multi-line and indented.

## A. Compilation fixes (source-observed; verify each with a full read before editing)
1. `tx/tests/apply_replicated_preflight_tests.rs:17-19`: `ApplyRejectReason` is NOT
   re-exported from `crate::tx` — import it from `crate::tx::apply_replicated::ApplyRejectReason`
   (the path other test files use).
2. `tx/tests/apply_replicated_journal_order_tests.rs`: (a) line ~41 uses `Store` without an
   import — add `shamir_storage::types::Store`; (b) moved-then-borrowed: `event_a`/`event_b`
   are moved into the batch at ~105-110 while later assertions borrow them (~126-128,
   136-137), and the pinned future borrows the temporary array. FIX: bind
   `let events = vec![event_a, event_b];` BEFORE `tokio::pin!(apply)`, pass `&events`, and
   build the downstream expectations from `&events[0]` / `&events[1]` afterwards. Re-read the
   whole test to make sure every value the future borrows outlives it and nothing is
   used-after-move.
3. `tx/tests/apply_replicated_tests.rs` (~line 478): `InMemoryRepo::store_get` requires the
   `Repo` trait — add it to the `shamir_storage::types` import (verify the trait path with a
   read of `shamir-storage/src/types.rs`).
4. `tx/tests/replication_interner_tests.rs`: the `apply_batch` helper (lines ~19-37) already
   awaits and unwraps and returns `ApplyOutcome`. Fix EVERY call site: remove the stale
   `.await.unwrap()` at ~86-88, ~125-133, ~315-317, and add `.await` where a caller passes
   the bare future into `assert_eq!(apply_batch(...), ApplyOutcome::...)` (~155-164,
   ~181-191, ~276-292). Read the WHOLE file and list every call site you fixed.
5. `tx/follower_durability/tests/crash_image_tests.rs`: add `use
   shamir_storage::error::DbResult;` (the `Store` impl returns it).
6. `FollowerBookmarkState` and `FollowerAdmission` need `#[derive(Debug)]` — wait: W1 owns
   those files. Do NOT add derives; instead check whether W1 already added them (read both
   files first). If W1 did NOT, add `#[derive(Debug)]` to `FollowerBookmarkState`
   (`meta/repl/bookmark_v3.rs:29`) and `FollowerAdmission` (`resume/activation.rs:20`) —
   `unwrap_err`/`expect_err` require `T: Debug`.
7. `activation_routing_tests.rs:89`: `InnerValue::Str("row")` → `InnerValue::Str("row".into())`
   (payload is `String`, see `shamir-types/src/types/value.rs:31,41`).

## B. Acceptance-coverage repairs (the fixtures must feed the code under test)
8. `activation_routing_tests.rs` — the apply-path fixtures currently write into a SEPARATE
   `info()` store, so the repository under test never sees them:
   - `legacy_bookmark_records_refuse_the_apply_repeatedly` (~166): write the legacy bytes to
     `repo.tx_info_store().await.unwrap()` (delete the standalone `info()` use in this test).
   - `floor_without_bookmark_refuses_follower_apply_repeatedly` (~181): seed the floor marker
     into the repo's own info store the same way.
   - `valid_v3_record_including_zero_bookmark_applies` and
     `v3_native_follower_applies_and_checkpoints_the_bookmark`: seed the v3 record into the
     repo's own info store (otherwise they only exercise fresh adoption, not the claimed
     native path).
   - `assert_legacy_bookmark_refusal` (~151): with a genuinely connected legacy record,
     `replication_bookmark()` returns the typed error — replace
     `assert_eq!(...unwrap(), 0)` with an assertion that it errs with
     `FollowerStateRefused { reason: LegacyBookmarkFormat }`.
   - NOTE (W1 interaction): gate initialization now REFUSES on a legacy record, so the
     pipeline fails inside `tx_gate()` with the SAME typed variant before admission — verify
     each assertion still matches the error variant (not the origin) and that the "no data /
     no bookmark / repeated" assertions hold. The data-unchanged assertion via
     `repo.get_table("items")` must keep working (get_table does not initialize the gate's
     legacy refusal) — verify with a read; if `get_table` on a legacy-seeded repo now fails,
     read the data through `repo.tx_info_store()`-independent means used elsewhere in the
     file or via the raw `__history__items` store pattern from `crash_subprocess_tests.rs`.
9. `crash_image_tests.rs::floor_only_crash_image_refuses_repeatedly_and_keeps_the_floor`
   (~160): `classify_follower_bookmark` returns `Ok(FollowerBookmarkState::FloorWithoutBookmark
   { floor: 7 })` for a floor-only image — that is CORRECT behavior (leaders must open). Fix
   the test to assert: classification = `Ok(FloorWithoutBookmark { floor: 7 })` on BOTH
   attempts, the FOLLOWER-ATTEMPT refusal is `save_fence_record_v3_aware(..)` →
   `Err(FollowerStateRefused { LegacyFollowerState })` on both attempts, floor preserved, and
   no v3 record appears. Rename to
   `floor_only_crash_image_classifies_unowned_and_refuses_adoption` and update the doc
   comment (an image is not a repo open; the open-side leader behavior is covered by
   `leader_with_floor_marker_still_opens_and_preserves_floor`).

## C. Native-branch capability test (acceptance test 8)
10. `activation.rs` cannot be handed a false-capability backend from tests (`BoxRepo` is a
    crate-internal enum — verify). Refactor for testability INSIDE the production file:
    extract the decision into
    `pub(crate) fn admission_decision(supports_atomic_batch: bool, state: FollowerBookmarkState) -> DbResult<FollowerAdmission>`
    implementing the W1-ordered logic (capability refuses EVERY state with
    `AtomicBatchUnsupported` first; then Native → Ok, Fresh → seed-state check → Ok,
    Floor → LegacyFollowerState refusal); `admit_follower` = classify + one call. Then, in
    `activation_routing_tests.rs`, add `admission_decision_refuses_unsupported_backends_before_any_state`:
    `(false, Native(valid v3))` → `Err(AtomicBatchUnsupported(_))`; `(false, Fresh)` → same;
    `(true, Native(_))` → `Ok(Native)`; `(true, Fresh)` → `Ok(Fresh)`; `(false, Floor...)` and
    `(true, Floor...)` → `Err(FollowerStateRefused { LegacyFollowerState })`. Document in the
    test why the false backend is exercised through the decision function (BoxRepo is not
    constructible with a refusing store from tests).

## D. Open-refusal-before-replay test (cold ordering)
11. In `tx/cold_reconcile/tests/` (folder has 5 entries; extend an EXISTING file —
    `reconcile_tests.rs` or `follower_cold_tests.rs`), add a fjall-gated test:
    seed a repo via `RepoInstance::from_factory` (fjall_raw tempdir), produce REAL in-flight
    WAL evidence (read `repo/tests/recovery_gap_marker_tests` / `tx/recovery_tests` for the
    established fixture pattern — e.g. a committed tx whose WAL entry survives close), then
    inject legacy bookmark bytes into `__tx__`, close, and reopen via `from_factory`:
    assert the open FAILS with `DbError::FollowerStateRefused { LegacyBookmarkFormat }`, and
    after the failed open a raw read of the stores shows NO replay/publication side effects:
    the durable `LastCommittedVersion` floor and the changelog clean horizon are unchanged,
    the legacy bookmark bytes are untouched, and no gap marker was written. Match the
    existing helpers' style (open_with_retry is NOT wanted here — the open must fail fast;
    assert the error directly).

## E. W1-interaction sweep
12. rg the engine tests for places that seed legacy/malformed bookmark bytes and then call
    ANYTHING that initializes the gate (`tx_gate()`, `apply_*`, `replication_bookmark()`):
    each must either expect the typed refusal or have seeded it intentionally. List what you
    checked in the report.

## Last step
`cargo fmt -p shamir-engine` (report the diff). NO other cargo command.

## Report
Per item: file:line of the fix; the full interner call-site list (item 4); the
get_table-on-legacy note (item 8); the decision-function signature (item 10); the open-refusal
test name + fixture pattern used (item 11); the E-sweep list; files over 900 lines; deviations.

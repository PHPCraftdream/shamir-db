# Wave F Post-Review — Independent Read-Only Audit

**Date:** 2026-07-26
**Reviewer:** Crush (independent post-wave review agent)
**Scope:** Wave F tasks F-1..F-15 (#791–#805) + follow-up #808
**Method:** `git show <sha>` full diffs for every Wave F commit; targeted
source reads of the current working tree; select `./scripts/test.sh` runs
(read-only verification, no code changes).

---

## Executive Summary

Wave F is **high quality**. All 15 tasks + the #808 follow-up close their
stated problems with real (non-vacuous) regression tests, accurate
documentation, and disciplined scope. The orchestrator's claim that each
fix was personally reviewed (full diff read + fmt/clippy/test gate) is
credible — the diffs are surgical, well-commented, and consistently cite
the mechanism they fix.

**No P0 or P1 correctness bugs were found.** All identified issues are P2
(design inconsistencies / SDK coverage gaps) or P3 (cosmetic / rare-path
leaks). The most actionable finding is **NF-1** (F-1 schema gate accepts
`Bin` but `Bin` can never use keyset seek — a semantic inconsistency, not
a row-loss bug).

**Test infra note (#807 context):** the `vr5_cofilter_sees_staged_and_filters_residual`
ANN test timed out across 5+ independent verification runs this wave
(F-2, F-6, F-9, F-10, F-13). Investigated as #807 and closed as
CPU-contention flakiness — **not a Wave F regression**. The test file
(`filtered_ann_tests.rs`) was never touched by any Wave F task. This is a
test-infrastructure reliability concern (a flaky test in the release gate),
not a product defect.

---

## Per-Task Assessment

| Task | Issue | Status | Comment |
|------|-------|--------|---------|
| **F-1** | #792 | ✅ Closed + **NF-1** | Schema gate correct for `Int`/`Bool`/`String`. `Bool` has a `compare_values` arm (resolve.rs:147) — fully functional. **`Bin` accepted by gate but `safe_seek_key` always returns `None` for it (cursor_handlers.rs:675)** — enters Keyset mode only to fall back to offset every FetchNext. No row loss, but gate's Bin acceptance is pointless. No positive keyset test for `Bool` or `Bin`. |
| **F-2** | #791 | ✅ Fully closed | Exact `cmp_i64_f64`/`cmp_u64_f64` unified across all 6 sites. NaN convention preserved (returns `None`). Cross-path regression test (`numeric_cmp_cross_path_tests.rs`) covers negative-fractional, 2^53, 2^63, 2^64 boundaries. `ScalarRef` has no `U64` variant (all int widths collapse to `Int`), so no gap in scalar_ref. |
| **F-3** | #793 | ✅ Fully closed | Rotate→consume→delete ordering verified on both login + sweep paths. Residual (consume-fails-after-rotate) is benign: file deleted + credential rotated, only meta stays active (cosmetic, self-healing on next sweep). E2e two-boot test is genuinely regression-catching. |
| **F-4** | #794 | ✅ Closed + **NF-3** | Validate→precompile→persist→activate correct in all 3 handlers. Rollback restores `rec_prev`. `parse_one_rule` silent-None fix is thorough (per-field tests). **Minor: `compile_table_schema`'s in-memory registry side-effects (register + add_binding) are NOT undone if activation fails after register succeeds — cosmetic leak, in-memory only.** |
| **F-5** | #795 | ✅ Fully closed | `Arc<AtomicBool>` gate, default false, checked before authz. KNOWN_LIMITATIONS §2 documents all 5 gaps accurately. **NF-4 (P3): gate-before-authz lets an unauthorized caller distinguish "disabled" from "denied" — intentional fail-fast, minimal impact.** |
| **F-6** | #796 | ✅ Fully closed | `&& !query.count_total` on `use_topk` gate. Comment rewrite accurate. Test covers ORDER BY DESC + LIMIT + count_total (both on/off). F-6∩F-7 interaction verified: both flags exclude from top-K, full-sort handles both simultaneously. |
| **F-7** | #797 | ✅ Closed + **NF-5** | Hard-rejects `with_version` + GROUP BY/agg/DISTINCT at request time (before count(*) shortcut — correct placement). `apply_order_by_qv_with_ids` shares permutation with id vector; `debug_assert_eq!` guards length. Tests verify ASC/DESC/pagination alignment against MVCC ground truth. **NF-5 (P2/test-gap): no test for ORDER BY + with_version + count_total triple combination.** |
| **F-8** | #798 | ✅ Fully closed | `serialize_u64` promotes to `Big(BigInt::from(v))` above `i64::MAX`. Bound check `v <= i64::MAX as u64` is exact. Differential test vs msgpack round-trip proves wire-shape parity. `u8/u16/u32` correctly left as `Int` (can't exceed i64::MAX). |
| **F-9** | #799 | ✅ Closed (documented residual) | `try_lock().is_ok()` probe verified: `fetch_next` holds `state().lock().await` across the ENTIRE scan (cursor_handlers.rs:1531, lock acquired before scan, dropped only on early-return/after bump_activity). Residual (collect→remove gap) is real but single-tick-sized — matches known follow-up #799-doc. |
| **F-10** | #800 | ✅ Closed + **NF-2** | All 14 `Err(_) => continue` sites now push `CorruptRecordRef`. ANN retry-loop `corrupt.clear()` per iteration is correct (avoids double-reporting on wider k′ rescan). `serde(default, skip_serializing_if)` backward-compatible. **NF-2 (P2): `corrupt_records` not exposed in Rust SDK or TS SDK — improvement invisible to SDK consumers.** Sibling files (table_manager_index_mgmt/streaming) documented out of scope. |
| **F-11** | #801 | ✅ Closed (documented follow-up) | small=128 MiB, medium=256 MiB (4× ratio). Headroom sanity-checked. `server.example.ktav` still missing the key — known open follow-up, explicitly out of scope. Verified: only small/medium `.ktav` have the key. |
| **F-12** | #802 | ✅ Fully closed | `copy_dir_recursive` re-opens dest write-only (no truncate) + `sync_all()`. `write_manifest` = create+write+sync. `fsync_dir` unix-only, logs-only on failure (matches WAL precedent). Windows fault-injection test (`#[cfg(windows)]`) is the closest portable proxy to crash-between-copy-and-rename. |
| **F-13** | #803 | ✅ Closed + **NF-6** | `try_build()` non-breaking sibling (build() unchanged). TS `build()` gains page=0/page_size=0 check. `Delete/Update/Upsert/AddSchemaRule/AlterSubscription::build()` now `Result`. **NF-6 (P2/residual): `IntoBatchOp` conversions still `.expect()` — the ~40 `Batch::*` ergonomic methods still panic on missing required field. Documented inline, accepted.** |
| **F-14** | #804 | ✅ Fully closed | `[0.1.0-alpha.1]` section created with accurate per-task bullets. Fresh empty `[Unreleased]` added above. Release-workflow grep gate verified. Every bullet cross-checked against the real diff — no inaccuracies found. |
| **F-15 + #808** | #805/#808 | ✅ Fully closed | `SecurityConfig::enable_experimental_migration_api` (default false), wired in `ServerLauncher::launch`. Shipped profiles stay disabled (config_tests guard). TS e2e harness updated. `ServerHandle::experimental_migration_enabled()` exposed for test assertions. CHANGELOG bullet added (#808). |

---

## New Findings

### NF-1 — F-1 schema gate accepts `Bin` but `Bin` can never use keyset seek

**Severity:** P2 (design inconsistency, no correctness impact)
**Files:**
- `crates/shamir-server/src/db_handler/cursor_handlers.rs:514` —
  `order_by_column_is_schema_typed_scalar` accepts `TypeTag::Bin`.
- `crates/shamir-server/src/db_handler/cursor_handlers.rs:675` —
  `safe_seek_key` returns `None` for `QueryValue::Bin(_)`.

**Root cause:** The F-1 schema gate accepts `TypeTag::Bin` because Bin is a
"non-container scalar" that's "homogeneous by construction." But homogeneity
alone doesn't make a column keyset-safe — the keyset boundary filter
(`field >= seek_key`) requires `compare_values` to have a matching arm.
`compare_values` (resolve.rs) has **no `Bin`/`Bin` comparison arm** — only
conversions (`FilterValue::Binary` → `QueryValue::Bin`). This is the exact
reason W-2 (#789) added the `QueryValue::Bin(_) => None` case in
`safe_seek_key`.

**Reproduction:**
1. Create a table with a schema declaring an `ORDER BY` field as `TypeTag::Bin`, `required: true`.
2. Insert ≥ page_size + 1 rows with distinct Bin values.
3. Open a cursor with `ORDER BY` that field, `page_size = 2`.
4. Observe: `pinned_mode` is `Keyset` (gate passed), but the second `FetchNext`
   extracts a `QueryValue::Bin` seek key → `safe_seek_key` returns `None` →
   per-call offset fallback.

**Impact:** No row loss (W-2's `safe_seek_key` prevents it per-call), but:
- The null probe runs unnecessarily for a column that will always fall back.
- The gate's claim that Bin is "safe for keyset" is misleading.
- `Bool` (also accepted) IS correctly comparable (`compare_values:147`) — but
  has no positive keyset test either.

**Recommendation:** Exclude `TypeTag::Bin` from the accepted set (same as
`F64`/`Dec`/`Big`), OR add a positive keyset test for `Bool` to document that
it genuinely works. `Bin` exclusion is the simpler, more honest fix.

---

### NF-2 — F-10 `corrupt_records` not surfaced in client SDKs

**Severity:** P2 (incomplete improvement)
**Files:**
- `crates/shamir-query-types/src/read/query_result.rs:130` — field exists,
  serialized with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`.
- `crates/shamir-sdk/` — no `corrupt_records` accessor or type.
- `crates/shamir-client-ts/src/` — no `corruptRecords` field in any response type.

**Impact:** A TS or Rust SDK client encountering corrupt records sees the
result set come back N rows short with **no signal** — identical to pre-F-10
behavior for SDK consumers. The improvement only benefits callers that
directly inspect the raw `QueryResult` struct (engine-internal / test code).

**Note:** This is not a regression (pre-F-10 behavior was the same for SDK
clients). It's an incomplete forward step. For an alpha release, acceptable —
flag for a follow-up.

---

### NF-3 — F-4 in-memory validator registry leak on activation rollback

**Severity:** P3 (cosmetic, in-memory only, extremely rare path)
**File:** `crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs:489-517`

**Root cause:** `compile_table_schema` performs 3 steps:
1. `validators.register(...)` or `replace_artifact(...)` — in-memory.
2. `table.add_validator_binding(...)` — in-memory + info-store write.
3. `validators.add_binding(...)` — in-memory.

If step 2 fails after step 1 succeeded, F-4's rollback restores the
catalogue (`rec_prev`) but does NOT undo step 1's in-memory registration.
The new `schema_validator_id` remains dangling in `ValidatorRegistry` until
server restart. It's never bound to any table (step 2 failed), so it can't
affect validation correctness — pure memory leak.

**Likelihood:** Near-zero. Step 1 only fails on validator_id collision
(unlikely for a fresh id); step 2 only fails if the table vanished between
the catalogue write and the bind (the table was just used for the write).

---

### NF-4 — F-5 gate-before-authz information disclosure

**Severity:** P3 (intentional, minimal impact)
**File:** `crates/shamir-db/src/shamir_db/execute/admin_migration.rs:40-58`

The experimental-migration gate runs before `authorize_access`. An
unauthorized caller gets `experimental_feature_disabled` (feature off) vs
`access_denied` (feature on) — distinguishing the two states. Intentional
("a disabled feature is not even an authz question"). Feature is always off
in production (no shipped profile sets it), so the practical exposure is nil.

---

### NF-5 — F-7 missing test for ORDER BY + with_version + count_total

**Severity:** P2 (test-gap, code path is correct)
**File:** `crates/shamir-engine/src/table/tests/with_version_order_by_tests.rs`

The triple combination `ORDER BY + LIMIT + with_version=true + count_total=true`
is handled correctly by the code (both flags exclude from `use_topk`, the
full-sort path threads ids via `apply_order_by_qv_with_ids` and
`apply_pagination` receives `count_total=true`), but no test exercises it.
A dedicated regression test would pin this interaction against future
refactors of the `use_topk` gate.

---

### NF-6 — F-13 IntoBatchOp still panics on missing required field

**Severity:** P2 (documented residual)
**Files:** `crates/shamir-query-builder/src/batch/into_batch_op.rs` —
`.expect()` at the `IntoBatchOp` boundary.

`Delete/Update/Upsert/AddSchemaRule/AlterSubscription::build()` now return
`Result`, but their `IntoBatchOp` impls (backing the ~40 `Batch::*` ergonomic
methods) still `.expect()` — a missing required field panics rather than
returning `BuilderError`. Documented inline on each impl as an accepted
tradeoff (IntoBatchOp is infallible-by-contract). Worth a follow-up to thread
`Result` through if the ergonomic API is public-facing.

---

## Cross-Task Interaction Analysis

| Interaction | Verdict |
|-------------|---------|
| **F-6 ∩ F-7** (both modify `use_topk` gate) | ✅ Correct. Both add their own `&& !flag`. A query with `count_total=true + with_version=true + ORDER BY + LIMIT` falls through to full-sort, which threads ids (F-7) AND computes total (F-6). No conflict. |
| **F-7 ∩ F-10** (both modify `read_collecting`) | ✅ Correct. Corrupt records are pushed to `corrupt` and `continue`d BEFORE `id_acc.push(id)` — the two vectors stay aligned. Composes cleanly. |
| **F-1 ∩ CR-B5** (schema gate vs cursor with_version rejection) | ✅ No interaction. Different layers (cursor mode selection vs request validation). CR-B5 rejects `with_version` at `CreateCursor`; F-1 affects only Keyset/Offset mode selection. |
| **F-1 ∩ W-2** (schema gate vs safe_seek_key) | ⚠️ **NF-1.** Schema gate accepts `Bin`; W-2's `safe_seek_key` rejects `Bin` per-call. Functionally safe (offset fallback) but semantically inconsistent. |
| **F-5 ∩ F-15** (gate + live-server wiring) | ✅ Correct. F-5 adds the gate; F-15 adds the only way to enable it from the live server. Consistent — no path bypasses the gate. |
| **F-3 login ∩ F-3 sweep** (both reorder rotation) | ✅ Correct. Both use rotate→consume→delete. Consistent error handling (best-effort, logged, non-fatal). |

---

## KNOWN_LIMITATIONS.md Consistency Check

The document is **accurate and up-to-date** as of Wave F:

- §2 (Schemas): migration API experimental gate + 5 gaps — all verified
  against the real code. F-15's config field is not mentioned by name (only
  `ShamirDb::enable_experimental_migration_api()`), but this is acceptable
  (the live-server wiring is an implementation detail of the gate).
- §6 (Results): keyset cursor state correctly reflects F-1's schema gate
  (mixed-type CLOSED for schema-typed Int/Bool/String/Bin; NaN MITIGATED via
  F64 exclusion). Corrupt-record reporting gap (F-10) accurately lists the
  two sibling files + `try_project_page_only_bytes` as out of scope.
- §7 (Numbers): `u64→Big` contract (F-8) and exact `Int↔F64` comparison (F-2)
  accurately documented.

**One documentation observation:** §6's corrupt-record bullet says
"all ~14 decode-failure sites in read_exec.rs" but doesn't mention that the
SDK layer doesn't surface the field (NF-2). Minor — the doc is about engine
coverage, not SDK exposure.

---

## CHANGELOG.md Accuracy Check

Cross-checked every Wave F bullet against the real diff. **All accurate.**

- F-1 through F-13 bullets match their diffs (function names, before/after
  behavior, file references).
- F-14's "Known limitations accepted for this release" closing bullet
  correctly cites §2 and §6.
- F-15 follow-up bullet (#808) accurately describes the config-field wiring.
- The `[0.1.0-alpha.1] - 2026-07-26` heading satisfies the release workflow's
  `grep -qE` gate.

No discrepancies found between CHANGELOG claims and actual code changes.

---

## Prioritized Remediation Plan

### Priority 1 — Low effort, improves correctness signaling

1. **NF-1: Exclude `TypeTag::Bin` from F-1 schema gate** (or add a positive
   keyset test for `Bool`). One-line change to the `matches!` in
   `order_by_column_is_schema_typed_scalar`, plus a test. Makes the gate's
   acceptance set honest.

2. **NF-5: Add ORDER BY + with_version + count_total triple test.** One test
   function in `with_version_order_by_tests.rs`. Pins the F-6∩F-7 interaction.

### Priority 2 — Medium effort, improves SDK consumer experience

3. **NF-2: Surface `corrupt_records` in TS SDK response type.** Add a
   `corruptRecords?: Array<{ table: string; id: ... }>` field to the TS
   `QueryResult` type. Rust SDK accessor if the SDK exposes `QueryResult`
   directly.

4. **NF-6: Thread `Result` through `IntoBatchOp`** (if ergonomic API is
   public-facing). Larger change — affects ~40 `Batch::*` methods. Assess
   whether the panic-on-missing-field contract is acceptable for the public
   API before committing.

### Priority 3 — Low priority, cosmetic

5. **NF-3: Undo `compile_table_schema`'s in-memory registration on rollback
   failure.** Add a `validators.unregister(id)` call in the rollback path.
   Rare-path cosmetic fix.

6. **NF-4: Move the F-5 gate after authz** (if the info-disclosure is deemed
   unacceptable). One-line reorder. Currently intentional — only change if
   the security model requires authz-first for ALL error codes.

7. **F-11 follow-up: Add `max_inflight_response_bytes` to
   `server.example.ktav`.** Documented open follow-up. One-line addition.

---

## Proposed Tasks

### Task A — F-1 follow-up: exclude `Bin` from schema-typed-scalar gate

**Priority:** P2 | **Depends on:** nothing

`order_by_column_is_schema_typed_scalar` (cursor_handlers.rs:514) accepts
`TypeTag::Bin`, but `safe_seek_key` (line 675) unconditionally returns `None`
for `QueryValue::Bin` because `compare_values` has no `Bin`/`Bin` arm. This
means a schema-declared `Bin` ORDER BY column enters `PaginationMode::Keyset`
only to fall back to offset on every `FetchNext` past page 1 — no row loss
(W-2's per-call fallback catches it), but the gate's acceptance is misleading.
Exclude `Bin` from the accepted `TypeTag` set (matching `F64`/`Dec`/`Big`),
update KNOWN_LIMITATIONS §6, and add a positive keyset test for `Bool` (which
IS correctly comparable via `compare_values:147` but currently has no positive
keyset test).

### Task B — F-10 follow-up: expose `corrupt_records` in TS SDK

**Priority:** P2 | **Depends on:** nothing

`QueryResult::corrupt_records` (query_result.rs:130) is serialized on the wire
but `shamir-client-ts` has no type or accessor for it. A TS client hitting a
corrupt record sees the result set come back short with no signal — identical
to pre-F-10 behavior for SDK consumers. Add an optional
`corruptRecords?: CorruptRecordRef[]` field to the TS `QueryResult` response
type, with a typed `CorruptRecordRef { table: string; id: ... }`. The wire
field is already `#[serde(skip_serializing_if = "Vec::is_empty")]`, so this
is purely additive.

### Task C — F-7 test-gap: ORDER BY + with_version + count_total triple test

**Priority:** P2 | **Depends on:** nothing

The `use_topk` gate in `read_collecting` (read_exec.rs:1070) excludes both
`count_total` (F-6) and `with_version` (F-7). A query with
`ORDER BY + LIMIT + with_version=true + count_total=true` falls through to
the full-sort path, which threads ids via `apply_order_by_qv_with_ids` and
passes `count_total=true` to `apply_pagination`. The code path is correct but
no test exercises the triple combination. Add one test in
`with_version_order_by_tests.rs` asserting both `versions` is populated AND
`pagination.total_count` is `Some(true_count)` for this combination.

### Task D — F-13 follow-up: fallible IntoBatchOp for public ergonomic API

**Priority:** P2 | **Depends on:** nothing (assess scope first)

`Delete/Update/Upsert/AddSchemaRule/AlterSubscription::build()` now return
`Result` (F-13), but their `IntoBatchOp` impls still `.expect()` — the ~40
`Batch::delete()/update()/upsert()` ergonomic methods panic on a missing
required field instead of returning `BuilderError`. If the ergonomic API is
public-facing, thread `Result` through `IntoBatchOp` (breaking change to the
trait — assess migration cost for the ~822 existing `build()` callers first).
If the ergonomic API is internal-only, the current `.expect()` contract is
acceptable as-documented.

### Task E — F-4 cosmetic: undo in-memory validator registration on rollback

**Priority:** P3 | **Depends on:** nothing

`compile_table_schema` (schema_management.rs:489) registers the validator in
`ValidatorRegistry` (step 1) before binding it (step 2). If step 2 fails after
step 1, F-4's catalogue rollback leaves the new `schema_validator_id`
dangling in the in-memory registry. Add a `validators.unregister(id)` (or
equivalent) in the rollback path of all three admin_schema handlers. Rare-path
cosmetic fix — the dangling entry is never bound and is cleared on restart.

### Task F — F-11 follow-up: add `max_inflight_response_bytes` to reference profile

**Priority:** P3 | **Depends on:** nothing

`deploy/server.example.ktav` (the reference/all-fields profile) still omits
`max_inflight_response_bytes`, deserializing to `None` (unbounded) despite
being the "document every field" profile. Small/medium profiles were fixed in
F-11; the reference profile was explicitly deferred. Add the key at the same
4× ratio (or a documented "set your own" comment) for consistency.

---

## Verification Runs Performed (read-only)

| Scope | Command | Result |
|-------|---------|--------|
| F-7 with_version tests | `./scripts/test.sh -p shamir-engine -- with_version_order_by` | exit 0 (passed) |
| F-10 corrupt record tests | `./scripts/test.sh -p shamir-engine -- corrupt_record` | exit 0, 5/5 passed |
| F-1 schema gate tests | `./scripts/test.sh -p shamir-server -- schema_typed` | exit 0, 3/3 passed |

No code was modified. No git mutations performed. Only `git show`/`git log`
(read-only) and `./scripts/test.sh` (existing test runner) were used.

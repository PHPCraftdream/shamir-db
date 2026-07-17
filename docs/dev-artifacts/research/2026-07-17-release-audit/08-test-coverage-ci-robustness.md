# 08 — Test coverage & CI robustness audit

Date: 2026-07-17. Read-only research pass (Read/Grep/Glob only; nothing executed).
Context: three bugs escaped the local suite and were only caught on real GitHub CI —
(1) over-strict SQ8 SIMD float tolerance on non-x86, (2) `$fn`+same-document-`$ref`
write-value regression with zero test coverage of the combination, (3) an MvccStore
whole-runtime deadlock that manifested only under CPU contention. This report hunts
for more instances of the same three classes plus general coverage/CI gaps.

## Executive summary

- **Category 1 (float fragility):** the SQ8 fix is done properly (atol+rtol) and the
  grep sweep found **no remaining pure-relative near-zero comparisons**. Residual
  risks are minor (exact `assert_eq!` on f32 in `sq8_tests.rs:101-105`, a stale
  "NEON is only type-checked" comment that is no longer true).
- **Category 2 (marker combinations):** the structural gap class that produced the
  `$fn`+`$ref` bug is **still open in three places**: top-level `$expr` in a write
  value has **zero tests** (despite being one of the 4 markers #641 claims),
  `$expr`+`$ref` / `$cond`+`$ref` behave *differently* from `$fn`+`$ref` (hard error
  vs pass-through) with **no test pinning either**, and `SetOp.key` markers are
  documented-resolved but never tested.
- **Category 3 (contention races):** ~60 files use `multi_thread` tokio tests (the
  nextest config's "33 files" note at `.config/nextest.toml:65` is stale). The
  MvccStore test family and one WAL group-commit assertion are the highest-risk
  narrow-window tests; most spin-waits have **no local timeout** and rely solely on
  the 180 s nextest kill, so a rare deadlock surfaces as an undiagnosable flaky
  TIMEOUT. Critically, `profile.ci`'s `test-threads = 4` *reduces* the very
  oversubscription that exposed the real deadlock — no CI job intentionally
  recreates contention.
- **Category 4 (zero-coverage surface):** BatchOp variants are well covered (all
  sampled variants have serde + dispatch + e2e tests). Real holes: the two
  proc-macro crates have zero direct tests, and **doctests are never run anywhere**
  (nextest cannot run them; raw `cargo test` is blocked by the perimeter guard) —
  at least one runnable doctest exists.
- **Category 5 (CI config):** four concrete issues: (a) **nextest version is
  unpinned in CI** despite the fail-closed `$NEXTEST` guard coupling that
  `.config/nextest.toml` explicitly warns must be re-verified on every upgrade;
  (b) the `test` job step lacks `shell: bash` on the Windows leg while the
  integration job deliberately sets it; (c) dev-box-tuned per-test kill overrides
  (SCRAM 60 s, WASM 240 s) leak into the CI profile that deliberately loosened the
  global to 600 s; (d) `cargo-cooldown` itself is installed unpinned.

---

## 1. Cross-platform-fragile numeric comparisons

Sweep method: grep for `1e-\d`, `EPSILON`, `approx`, `assert_relative`,
`abs() /`, `rel_err` across `crates/` (tests and non-tests).

### Verified-fixed (the SQ8 class)

- `crates/shamir-index/src/vector/tests/sq8_tests.rs:222` and `:273` now use the
  combined bound `tol = 1e-3 * ref.abs() + 1e-5` with an explicit comment citing
  the exact CI failure (dim=8 seed=17, non-x86: ref ≈ 1.35e-4, diff ≈ 1.9e-6,
  1.4 % relative). Correct form.
- `crates/shamir-index/src/vector/tests/simd_tests.rs:161-167` — `assert_close`
  uses `tol = 1e-3 * want.abs().max(1.0)`: an absolute floor of 1e-3, safe for
  near-zero references.
- `crates/shamir-index/src/vector/tests/sq8_tests.rs:173-175` — median-of-relative
  with an explicit `truth.abs() > 1.0` near-zero skip. Robust.

### Remaining assertions — audited, mostly safe

- `crates/shamir-index/src/vector/tests/quantized_dist_tests.rs:118,144,170,252,314,351`
  — all **absolute** error bounds (`max_abs_err < 1e-3` etc.), which do not blow up
  at near-zero references. Safe form.
- `crates/shamir-engine/src/tx/tests/tx_vector_delete_tests.rs:557,585,601` —
  `entry.abs() < 1e-5` on cosine distance of a vector against itself
  (mathematically exactly 0 for exactly-representable unit-basis vectors). Correct
  absolute form for a near-zero expectation.
- `crates/shamir-engine/tests/crash_recovery.rs:1120`,
  `crates/shamir-types/src/codecs/interned/tests/messagepack_tests.rs:520`,
  `crates/shamir-query-types/src/read/tests/query_record_tests.rs:219`,
  `crates/shamir-query-types/src/filter/tests/filter_value_conv_tests.rs:57` —
  absolute bounds on serde round-trips (bit-identical by construction). Safe.
- `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs:810-1068` — 1e-4
  absolute on stored-f32 round-trips. Safe.

### Residual (low-severity) flags

1. **Exact f32 `assert_eq!` on computed scales** —
   `crates/shamir-index/src/vector/tests/sq8_tests.rs:101-105`
   (`assert_eq!(scales[0], 10.0 / 255.0)`). Passes today because `fit()` computes
   the same scalar expression; if `Sq8Quantizer::fit` is ever vectorized or
   reassociated (FMA), this becomes the next non-x86 CI-only failure. Cheap fix:
   half-ulp tolerance or a comment pinning the "same scalar expression" contract.
2. **Bit-equality asserts** —
   `crates/shamir-index/src/vector/tests/quantized_dist_tests.rs:204-205,227,320-324`
   (`to_bits()` equality). Same-process/same-code-path today, but the
   `rescore_f32`-wrapper-vs-`RescoreCtx::score` bit-equality at `:320` silently
   encodes "the wrapper must never take a different codegen path". Acceptable, but
   it is the kind of assert that fails only on an exotic target.
3. **Stale platform comment** — `crates/shamir-index/src/vector/tests/simd_tests.rs:9-12`
   claims the NEON paths are "type-checked via `cargo check --target aarch64-*`"
   only. Since `macos-latest` runners are Apple Silicon, the CI matrix
   (`.github/workflows/ci.yml:55,105`) now actually *executes* the NEON kernels —
   that is precisely how bug (1) was caught. Update the comment so nobody
   "optimizes away" the macOS matrix leg believing NEON is untested anyway. Note
   there is still **no Linux-aarch64 leg**: macOS is the *only* NEON executor.

**Conclusion:** no other instance of the near-zero-reference pure-relative
tolerance class exists in the test suite today.

---

## 2. Write-value marker combinations (`$param`/`$query`/`$fn`/`$cond`/`$expr`)

Resolver: `crates/shamir-engine/src/query/batch/param_subst.rs`.
Test file: `crates/shamir-engine/src/query/batch/tests/executor_tests/write_value_resolution_tests.rs`.

### What IS covered (citations)

| Combination | Test |
|---|---|
| `$query` in `InsertOp.values` | `insert_value_with_query_ref_resolves_to_real_value` (:35) |
| `$query` in `UpdateOp.set` | `update_set_value_with_query_ref_resolves_to_real_value` (:95) |
| `$query` in `SetOp.value` | `upsert_value_with_query_ref_resolves_to_real_value` (:146) |
| `$fn` literal-arg | `insert_value_with_fn_call_literal_arg_resolves` (:200) |
| `$fn`+`$ref` pass-through (unit ×2 + e2e sibling combo) | :252, :273, :319 |
| `$cond` true/false branch (ValueCompare condition) | :381, :412 |
| `$query` unknown alias → plan-time error | :447 |
| malformed `$fn` payload → hard error | :476 |
| `$param` bind / unbound / plain-literal fast path | :510, :547, :577 |

### Structural gaps — same class as the shipped `$fn`+`$ref` bug

1. **Top-level `$expr` in a write value: ZERO tests.** The module doc
   (`param_subst.rs:1-7`) and the test-file header (:1) both name `$expr` as one of
   the four #641 markers, but no test in the workspace exercises
   `{"field": {"$expr": {...}}}` as a write value (grep across all tests: `$expr`
   appears only *nested inside* the `$fn` pass-through test at :280). A regression
   in the `Expr` msgpack round-trip (`param_subst.rs:224-232`) or in
   `resolve_filter_query`'s Expr arm would ship exactly like the `$fn`+`$ref` bug
   did. **This is the most direct mirror of the escaped bug.**
2. **Asymmetric `$ref` handling is unpinned.** The pass-through-to-table-layer
   check fires **only** for `FilterValue::FnCall` (`param_subst.rs:244-248`).
   The table layer only recognizes `$fn` (`crates/shamir-engine/src/table/write_helpers.rs:71-76`,
   `is_computed_field` checks `m.contains_key("$fn")`). Consequences:
   - `{"$expr": {"op":"add","args":[{"$ref":["a"]},{"$ref":["b"]}]}}` — the exact
     example used in `param_subst.rs`'s own doc comment (:25-27) — resolves against
     `DUMMY_RECORD = InnerValue::Null` (:142), the `FieldRef` misses, and the write
     **hard-errors** with `MalformedMarker`. Fail-closed, arguably fine — but no
     test asserts this, and no user-facing doc distinguishes "computed field must
     use `$fn`, not `$expr`".
   - `$cond` whose *condition* references record fields (not `ValueCompare`)
     evaluates the condition against the Null record and **silently picks a
     branch** — a silent-wrong-value risk, untested (`filter_value_contains_field_ref`
     at :76-85 conservatively flags any `Cond`, but that only matters when nested
     inside a `$fn`'s args).
3. **`SetOp.key` markers untested.** `param_subst.rs:157` documents
   `SetOp.{key,value}` as resolved; the only upsert test uses a literal
   `key(mpack!({...}))` (`write_value_resolution_tests.rs:170`). A marker in the
   upsert *key* (the row-identity path) has no coverage at all.
4. **Nesting combinations untested.** The resolver recurses through Maps/Lists
   (`param_subst.rs:257-270`), but no test puts a marker inside a nested object, a
   List element, a `$cond` branch that is itself a `$query` ref, or a `$fn` arg
   that is a `$param`. Dependency extraction for `$query` refs *nested inside*
   `$fn` args / `$cond` branches of a write value is likewise unexercised (only the
   top-level unknown-alias case at :447).

**Recommendation:** one test file addition covering (a) top-level `$expr` literal
resolution, (b) top-level `$expr`+`$ref` → pinned hard error (or a decision to
extend pass-through symmetrically), (c) `$cond` with record-field condition →
pinned behavior, (d) marker in `SetOp.key`, (e) a marker two levels deep inside a
nested map + inside a list element.

---

## 3. `multi_thread` contention-window tests

`grep multi_thread` over `crates/` (excluding benches/examples/binaries) finds
**~60 test files**, not the 33 noted in `.config/nextest.toml:65` — that comment is
stale; the worker-thread oversubscription it reasons about is roughly 2× worse than
its arithmetic assumes.

### Highest-risk narrow-window tests (same family as the caught deadlock)

- **`crates/shamir-tx/src/tests/mvcc_store_tests/overlay_ordering_tests.rs:114,213`**
  — the known pair: 4 reader tasks in `while !stop.load()` / `yield_now` spin loops
  (:140, :237) racing a writer through the real ack path. The reader loops have
  **no local timeout**; if the writer deadlocks (the actual bug class), the test
  hangs until nextest's 180 s kill and reports an anonymous TIMEOUT.
- **`crates/shamir-tx/src/tests/mvcc_store_tests/a10_toctou_tests.rs:96`** —
  2-worker race of `open_snapshot` vs `vacuum_key` through a `PausableStore` gate;
  deterministic window, but the same store/gate machinery as the deadlock. Sibling
  files `cell_reservation_tests.rs`, `stream_tests.rs`, `retention_tests.rs`,
  `publish_monotonic_tests.rs`, `pending_ts_race_tests.rs` all race MvccStore
  windows on 2–4 workers.
- **`crates/shamir-wal/src/tests/wal_group_commit_tests.rs:107`
  (`synced_fsyncs_are_batched`)** — asserts `fsyncs < 32` after 32 concurrent
  Synced appends (:132-137). Coalescing is **timing-dependent**: under total CPU
  starvation (4-core CI runner, saturated) every append can land in its own commit
  window → 32 fsyncs → false failure. This is a contention-*flake* (not deadlock)
  waiting for a busy CI day. The sibling `buffered_only_window_issues_no_fsync`
  (:140) is explicitly designed to be timing-independent — the batching test is not.
- **`crates/shamir-engine/src/repo/group_commit/tests/leader_cancel_tests.rs:26`**
  — leader-cancellation mid-commit on 4 workers; the exact leader/follower
  hand-off shape that deadlocks when a notification is lost. Worth a loop-under-load
  soak when the group-commit code is next touched.
- **Spin-waits with no local timeout** (backstopped only by the 180 s nextest
  kill, so a rare hang = undiagnosable flaky TIMEOUT):
  `overlay_ordering_tests.rs:140,237`;
  `crates/shamir-engine/src/table/tests/doctor_tests.rs:214-251`;
  `crates/shamir-engine/src/tx/tests/backpressure_gc_tests.rs:284`;
  `crates/shamir-tx/src/tests/mvcc_store_tests/version_tests.rs:606`;
  `crates/shamir-tx/src/tests/mvcc_store_tests/lock_tests.rs:165,178`.
  Contrast: `crates/shamir-index/src/vector/tests/quantized_graph_tests.rs:1630-1639`
  does it right — the spin is wrapped in an explicit 5 s `tokio::time::timeout`
  with a named assertion message. That pattern should be the template.
- **Barrier audit:** every async test uses `tokio::sync::Barrier` (verified:
  `ssi_stress_tests.rs:21`, `oracle_stress_tests.rs:543`, `acceptance_tests.rs:606`,
  `compaction_tests.rs:263`, `hnsw_adapter_tests.rs:508`, `group_tests.rs:276`).
  The single `std::sync::Barrier` (`crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:208`)
  is the documented Argon2 OS-thread fix. No sized-greater-than-workers std-barrier
  deadlock exists.

### The structural problem

`.config/nextest.toml:80` sets `profile.ci` `test-threads = 4` precisely to
*reduce* worker-thread oversubscription on CI runners. That is correct for
stability — but oversubscription is exactly the condition that exposed the real
MvccStore deadlock (bug 3). After this cap, **no environment routinely recreates
the contention that found the bug**: dev boxes have 16 idle cores, CI is now
throttled. Recommendation: add a scheduled (nightly/weekly) stress job that runs
`./scripts/test.sh @oracle --full` plus the mvcc_store_tests with deliberately
high parallelism (e.g. `--test-threads` 2–3× cores, or the suite looped 10×), so
contention-only deadlocks have a home where they are *expected* to reproduce.

---

## 4. General coverage gaps

### BatchOp surface — good

All sampled variants have real coverage: `RenameIndex`
(`crates/shamir-db/tests/rename_index_e2e.rs`,
`crates/shamir-engine/src/table/tests/index_rename_concurrency_tests.rs`),
replication DDL (`crates/shamir-query-types/src/admin/types/tests/repl_ops_tests.rs`,
`crates/shamir-query-builder/tests/repl_ddl_msgpack.rs`,
`crates/shamir-db/src/shamir_db/tests/replication_ddl_tests.rs`), interner ops
(`crates/shamir-db/src/shamir_db/tests/interner_ops_tests.rs`), temporal ops
(`crates/shamir-db/tests/changes_since.rs`, `temporal_e2e.rs`). The exhaustive
no-wildcard `match` design in `batch_op.rs::is_write` (:660) and
`::required_access` (:468) forces classification of new variants at compile time —
a genuinely good guard.

### Real holes

1. **Doctests never run, anywhere.** The perimeter guard blocks raw `cargo test`
   (the only doctest runner), and cargo-nextest does not support doctests. There
   are 220 doc code fences across `crates/*/src`; most are ` ```text `/` ```ignore `,
   but at least one is a runnable ` ```rust ` doctest —
   `crates/shamir-query-builder/src/query/query.rs:281` — which is therefore never
   compiled or executed by any gate (clippy `--all-targets` does not build
   doctests either). It can silently rot into a non-compiling example. Fix: either
   a narrow CI step `cargo test --doc -p shamir-query-builder` (would need a guard
   exemption) or demote the fence to `ignore` with a mirrored unit test.
2. **Proc-macro crates have zero direct tests:** `shamir-query-builder-macros`,
   `shamir-sdk-macros` (0 test-containing files each). They are exercised only
   indirectly through their consumer crates; there are no trybuild/UI tests pinning
   error messages or edge-case expansions.
3. **`shamir-collections` has zero tests** — acceptable (63-line alias crate,
   `crates/shamir-collections/src/lib.rs`), noted for completeness.
4. **Node e2e suite never runs in CI** — documented at
   `.github/workflows/ci.yml:90-95`; the promised nightly promotion is still
   outstanding. Combined with `shamir-client-node` being outside the workspace,
   the napi binding has zero automated coverage on any cadence.

---

## 5. CI configuration health

Files: `.github/workflows/ci.yml`, `.config/nextest.toml`, `scripts/test.sh`.

### Strengths (for the record)

Clippy on all 3 OSes with `--locked` (ci.yml:28-41, with the correct rationale
that cfg-gated code is only linted on its OS); lib-test AND integration matrices on
all 3 OSes (:49-61, :99-117); `macos-latest` = Apple Silicon gives real NEON
execution; `fail-fast: false` everywhere; a fail-closed dependency-cooldown gate
(:119-133); `scripts/test.sh` auto-selects `--profile ci` when `CI=true`
(test.sh:196-198).

### Issues

1. **nextest version unpinned vs. the fail-closed guard coupling.**
   `.config/nextest.toml:9-21` documents that the entire test perimeter hinges on
   nextest setting `$NEXTEST`, pins baseline **0.9.137**, and instructs "when
   upgrading nextest, confirm `$NEXTEST` is still set before merging". But both CI
   test jobs install via `taiki-e/install-action@nextest` (ci.yml:59,109) with
   **no version**, i.e. always-latest. A nextest release that renames `$NEXTEST`
   would fail-closed — refusing ALL test invocations across the matrix — with no
   local reproduction (dev boxes keep their pinned install). This is the exact
   risk the config file warns about, unmitigated in CI. Fix:
   `tool: cargo-nextest@0.9.137` (install-action supports pinning) or a version
   assertion in `scripts/test.sh`.
2. **`test` job step is missing `shell: bash` on Windows.** ci.yml:61
   (`- run: ./scripts/test.sh --locked`) has no `shell:` override, so on
   `windows-latest` it runs under the default `pwsh`, which cannot natively execute
   a `.sh` path. The integration job explicitly adds `shell: bash` with the comment
   "bash on every runner (Git Bash on Windows)" (ci.yml:111-117) — the authors
   clearly knew it was needed. Either the Windows lib-test leg errors (check a real
   run log) or it works by an undocumented file association; both are fragile.
   Fix: add `shell: bash` to the `test` job step for parity.
3. **Dev-box-tuned kill thresholds leak into the CI profile.** nextest overrides
   from `[[profile.default.overrides]]` apply as fallback under `--profile ci`.
   So in CI: SCRAM tests keep the 10 s×6 = **60 s kill** (nextest.toml:87-89,
   Argon2-bound, tuned for 16-core dev boxes) and `wasm_function_*` keeps
   120 s×2 = **240 s kill** (:83-85, "~99 s legit" *on a dev box*) — even though
   `profile.ci` deliberately loosened the global to 60 s×10 = 600 s (:54-57)
   because "CPU-bound tests can legitimately need much longer wall-clock on CI".
   The two loudest CPU-bound families are exactly the ones excluded from that
   loosening. On a saturated 4-core runner a legit 99 s WASM compile can stretch
   3–4× and hit the 240 s kill. Fix: add matching `[[profile.ci.overrides]]` with
   proportionally looser budgets.
4. **`cargo-cooldown` itself is unpinned.** ci.yml:132
   (`cargo install --locked cargo-cooldown`) — `--locked` only honors the tool's
   own lockfile; the *version* floats. The supply-chain gate is itself subject to
   supply-chain drift. Fix: `cargo install --locked cargo-cooldown@<ver>`.
5. **No contention/stress lane** (see §3): `profile.ci test-threads = 4`
   (nextest.toml:80) suppresses the oversubscription that exposed the MvccStore
   deadlock; no scheduled job restores it.
6. Minor: stale "33 files" arithmetic in nextest.toml:65 (now ~60);
   `wasm-heavy max-threads = 4` (:35) equals the CI-global `test-threads = 4`, so
   the group provides no throttling relative to the rest of the suite on CI
   runners; no `cargo doc` / feature-matrix / MSRV lane (unverified whether any
   feature-gated code exists); stray `fix643_test.log` at the repo root (test-run
   debris that should not land in a commit).

---

## Prioritized recommendations

1. Pin cargo-nextest in CI to the guard-verified baseline (§5.1). One-line change,
   prevents a matrix-wide fail-closed outage.
2. Add the §2 marker-combination test file — top-level `$expr` (resolve + `$ref`
   error pinning), `$cond`-with-record-condition pinning, `SetOp.key` marker,
   deep-nesting cases. This closes the still-open half of the bug-2 class.
3. Add `shell: bash` to the `test` job step (§5.2) and `profile.ci` overrides for
   SCRAM/WASM (§5.3).
4. Add a scheduled contention-stress lane for `@oracle` + mvcc_store_tests (§3).
5. Wrap the unbounded test spin-waits in explicit `tokio::time::timeout` with named
   messages, using `quantized_graph_tests.rs:1630` as the template (§3).
6. Decide the doctest policy (§4.1) and pin `cargo-cooldown`'s version (§5.4).

# Brief — S5c-X PR6b-2a Round 8: shadow-log dead code, read-only resolver, layout

Read `CLAUDE.md`, `docs/dev-artifacts/prompts/remediation/113-s5cx-round7.md`, and the Round 7 reviewer verdict. NO builds/tests/clippy/rustc; NO git-mutating commands; the orchestrator runs `cargo fmt` afterwards — do not run it. Read REAL definitions before editing; existing tests keep their meaning.

## D1 — CRITICAL, DO FIRST: dead shadow-log write path (CI `-D dead_code`)

CI now fails: `MigrationShadowLog::{append, append_batch, entry_key}`, `encode_entry`, and `MigrationCoordinator::shadow_log()` are NEVER USED. A dead `append` in a migration path can mean the shadow log is no longer written — a lost-behaviour regression hiding behind a lint.

MANDATORY INVESTIGATION FIRST (read-only):
- `git grep -n "shadow_log\|ShadowLog\|\.append(\|append_batch" origin/master -- crates/shamir-db crates/shamir-server crates/shamir-engine` and read EVERY master call site of `MigrationShadowLog::append` / `append_batch` / `MigrationCoordinator::shadow_log()`.
- Establish: in master, WHO wrote shadow entries during an active migration (coordinator internals? a facade write path? a TableManager hook?), and WHEN (source writes during snapshot/drain).
- Establish what the current branch does instead on the same path. Two possible outcomes:
  (a) **Lost behaviour**: master's writes relied on `shadow_log()`/`append` and the Rounds 5-7 visibility demotions deleted the only callers → RESTORE the shadow writes on the ADMITTED engine path (e.g. an admitted twin on `TableManager`/`MigrationCoordinator` that debug-checks permits and calls `append`/`append_batch`), with the master call-site semantics preserved. Add/keep a deterministic test proving a source write during an active migration lands in the shadow log (assert via `shadow_lag`/`drain_until_caught_up`), or extend an existing migration test if one already covers drain of live writes — if coverage exists and passes through the restored path, cite it instead of writing a new one.
  (b) **Truly obsolete**: master ALSO had no live writer for these specific functions on any path that matters (e.g. `append` was only used by a removed facade duplicate that itself never ran, and live shadowing goes through a different existing call chain — name the live chain and prove it ends in `read_from`/`shadow_lag` consuming entries). Then DELETE the dead items (`append`, `append_batch`, `encode_entry`, `entry_key` if truly orphaned, `shadow_log()` accessor if orphaned — keep whatever `drain_shadow_log`/`purge`/recovery still need) and say exactly why nothing was lost, citing the live write chain.
- Your final message and the brief's `## Round 8 inventory` MUST state, per dead item: lost behaviour or obsolete, with the master file:line evidence and the current-chain evidence.

## D2 — `resolve_filter_value` still mutates the dictionary through a public resolver (P1)

`crates/shamir-engine/src/query/filter/resolve.rs:695-701`: `pub fn resolve_filter_value` (re-exported at `query/filter/mod.rs:33-36`) ends with `query_value_to_inner(&qv, ctx.interner)` — mutating (`touch_ind`) on caller-controlled data (a `FilterValue::Param` resolves to caller-supplied params, `resolve.rs:376-380`). The read-view `filter_context` hands out such a context.

FIX: make the conversion READ-ONLY — replace the `query_value_to_inner` call with `query_value_to_inner_with` using a closure that resolves each name via `interner.get_ind(name)` and returns a `CodecError` (e.g. `Decode` "field name not interned: …") when absent. No new interner ids, no admission needed. Also GREP for any OTHER public fn in `crates/shamir-engine` that converts with `ctx.interner` or otherwise calls `touch_ind`/`touch_with_id` on a context-borrowed interner (`grep -rn "query_value_to_inner\|touch_ind\|touch_with_id" crates/shamir-engine/src/query crates/shamir-engine/src/table/write_receipt/admission/read_only_views.rs`) and fix every public-reachable mutation the same way (private/engine-internal write paths may keep the mutating form). Update `read_only_views.rs` doc claims if needed (they should already be true after this).

## D3 — P2: boundary doctests for the context capability

Add `compile_fail` doc blocks (alongside the existing four on `TableManager`, `table_manager.rs:52-90`, or on `FilterContext`'s doc in `eval_context.rs`) proving downstream cannot reach mutators through the read-view capability, one block per route:
- `ctx.interner` field access (pub(crate) → must not resolve downstream): e.g. `fn f(ctx: &shamir_engine::query::filter::FilterContext) { let _ = ctx.interner.touch_ind("x"); }`
- `InternerReadView`'s private cell / absent mutators: `fn f(v: &shamir_engine::table::InternerReadView) { v.touch_ind("x"); }` (no such method → must not compile).
These run in CI via the existing `cargo test --doc -p shamir-engine` steps. Keep the existing four blocks intact.

## D4 — LAYOUT: `crates/shamir-db/src/shamir_db/system_store.rs` 1181 → 1198 (over-limit file grew)

Net zero or negative vs 1181 (shrink ≥17 lines). Cleanest: merge `persist_table_interner` (:162-170) and `persist_table_interner_interning` (:179-194) into ONE helper, e.g. `async fn persist_table_interner_impl(&self, table: &TableManager, intern: Option<&QueryValue>)` with two thin `pub(crate)` wrappers, or give the 16+2 call sites a single function taking `Option<&QueryValue>` — whichever is smaller and clearer. Do NOT delete the doc substance; tighten instead. Verify the final line count and state it.

## Sequence
D1 investigation (write findings into `## Round 8 inventory` BEFORE editing) → D1 fix → D2 → D4 → D3. Then re-read every changed file end-to-end (compile-readiness: use resolution, arities, dead-code recheck — after D1/D2 grep that NO previously-flagged item became newly dead and no new `-D warnings` trap appears: unused imports, needless borrows, private_interfaces). Append the inventory + per-dead-item verdicts to THIS brief. Final message: per-file changes with file:line, the dead-item verdicts with evidence, items unverified. Do NOT run cargo fmt; do NOT commit.

## Round 8 inventory

### D1 investigation (read-only, done BEFORE editing)

Master call sites (`git grep` against origin/master, all read directly):
- `crates/shamir-engine/src/migration/shadow_log.rs` master: `append` pub (:126), `append_batch` pub (:135); `entry_key` (:203) and `encode_entry` (:44) are called ONLY from `append`/`append_batch` bodies (:129-132, :146-150).
- WHO wrote shadow entries in master: NOBODY in production. Every master call of `.append(`/`append_batch` on a `MigrationShadowLog` is in crate-internal test mods: `coordinator_tests.rs:60,78,186,486,495,501,582,647`, `shadow_log_tests.rs` (throughout), `cutover_guard_tests.rs:63,83,136`. All other `.append(`/`append_batch` hits are the unrelated WAL `DurabilityPolicy` tests. Production code only CONSTRUCTS the log (`admin_migration.rs:172` fresh-start, `:301-311` recovery) and coordinator.rs only reads/drains/purges it (`drain_shadow_log` master:359 → `read_from` :371 → `apply_shadow_entries`; `purge` :480,496).
- WHEN/why no writer: commit `0c59f555` ("feat(migration): freeze writes to the source table during online migration (#1220)") records the explicit design-review decision NOT to build write interception: "do not build write interception (4-5 PRs, little value while the destination is in-memory only...) The source is quiesced instead; a write answers the typed error `table_migrating`." So during an active migration there are no source writes to shadow, by design.
- `MigrationCoordinator::shadow_log()`: master coordinator.rs:254 (pub) — ZERO callers on master anywhere (grep `.shadow_log()` across master crates returns no call sites); also zero on this branch (coordinator.rs:318, pub(crate)).
- Live chain on this branch that consumes entries (kept, untouched): `drain_shadow_log` (coordinator.rs:436) → `read_from` (shadow_log.rs:157) → `apply_shadow_entries` (coordinator.rs ~:407); `purge` (:583,:600); `recover` (shadow_log.rs:94); used by `admitted_ops.rs:231,270` (coordinator factories) and `crates/shamir-db/tests/migration_index2.rs:439`.
- The CI dead-code set appeared because Round 6 (commit 41d5fb86) demoted these from `pub` (dead-code-invisible in a lib) to `pub(crate)`/private, exposing that they had no non-test callers — on master OR branch.

Per-dead-item verdicts:
- `append` — OBSOLETE (not lost behaviour): no master production writer ever existed; source writes are frozen during migration by design (0c59f555). Kept as a `#[cfg(test)]` seeding helper (engine tests seed the shadow log through it) with a doc note pointing at the deferred write-interception decision. Fix: `#[cfg(test)]` gate.
- `append_batch` — OBSOLETE, same evidence and same fix (test seeding helper only).
- `encode_entry` — OBSOLETE: helper of append/append_batch only; gated `#[cfg(test)]` alongside.
- `entry_key` — OBSOLETE: helper of append/append_batch only; gated `#[cfg(test)]` alongside.
- `MigrationCoordinator::shadow_log()` — OBSOLETE: zero callers on master and on branch; DELETED.

### Round 8 per-file changes

- `crates/shamir-engine/src/migration/shadow_log.rs`
  - :44-61 `encode_entry` → `#[cfg(test)]` + Round 8 doc (test seeding helper).
  - :131-146 `append` → `#[cfg(test)]` + doc citing master zero-caller evidence and 0c59f555.
  - :151-155 `append_batch` → `#[cfg(test)]` + doc.
  - :222-226 `entry_key` → `#[cfg(test)]` + doc.
- `crates/shamir-engine/src/migration/coordinator.rs`
  - :315-317 (pre-edit :315-320) DELETED `pub(crate) fn shadow_log()` — zero callers on master and branch.
- `crates/shamir-engine/src/query/filter/resolve.rs`
  - :6-8 imports: `query_value_to_inner` → `query_value_to_inner_with` + `shamir_types::codecs::CodecError` (root re-export; `codecs::error` module is private).
  - :689-703 doc: Round 8 read-only note.
  - :710-717 `resolve_filter_value` body → `query_value_to_inner_with(&qv, &|name| ctx.interner.get_ind(name).ok_or_else(|| CodecError::Decode(format!("field name not interned: {name}")))).ok()` — no new ids, fails closed.
  - D2 sweep: all other engine `touch_ind`/`touch_with_id`/`query_value_to_inner` sites (aggregate.rs:1167,1206; exec.rs:194 pub(crate); read_index_scan.rs:138; table_manager_* / write_exec / tx/* / admitted_ops.rs:204) take an engine-owned interner on write/admitted/internal-read paths — none reachable through a read-view `FilterContext`/`InternerReadView`. `intern_field_path`/`intern_field_path_compact`/`compile_filter` are `get_ind`-only (resolve.rs:50-82). read_only_views.rs doc updated (:13-17, :71-73).
- `crates/shamir-engine/src/table/table_manager.rs`
  - :91-113 two new `compile_fail` doctests on the existing gate doc (kept the four existing blocks intact): `ctx.interner.touch_ind` (pub(crate) field) and `InternerReadView::touch_ind` (no such method).
- `crates/shamir-db/src/shamir_db/system_store.rs`
  - :157-188 `persist_table_interner` + `persist_table_interner_interning` merged into ONE `pub(crate) async fn persist_table_interner(&self, table, intern: Option<&QueryValue>)` (single admission, optional `intern_query_value_admitted`, then `persist_interner_admitted`); 13 in-file call sites updated (:249, :312, :434, :504, :575, :679, :911, :940, :973, :997, :1063, :1088, :1158); the 22 `// =====` banner rails stripped (titles kept) to get under the hard ≤1181 gate.
  - **Final line count: 1162 ≤ 1181.**
- `crates/shamir-db/src/shamir_db/execute/admin_replication.rs` — 5 call sites → `(&table, None)` (:106, :232, :351, :535, :809).
- `crates/shamir-db/src/shamir_db/execute/replication_seed/catalogue.rs` — 2 call sites → `(&table, None)` (:91, :129).
- `crates/shamir-engine/src/table/write_receipt/admission/admitted_ops.rs` — :195 stale doc ref to the removed `persist_table_interner_interning` renamed (found in the re-read; minimal fix).
- `crates/shamir-engine/src/table/write_receipt/admission/read_only_views.rs` — :13-17 Round 8 claim, :71-73 stale doc ref fixed.

### Round 8 compile-readiness re-read + dead-code recheck
- No remaining non-test references to `append`/`append_batch`/`entry_key`/`encode_entry` (all callers are engine `cfg(test)` mods, which still see them), no references to the deleted `shadow_log()` accessor, no references to `persist_table_interner_interning` (incl. docs), no unused imports (resolve.rs still uses `Interner`, `inner_value_to_query_value`, `InternerKey`; `CodecError` re-exported at `shamir_types::codecs` root since `codecs::error` is private).
- No previously-flagged dead item became newly dead; the only new `cfg(test)`-only items are the four deliberately gated shadow-log seeding helpers. `next_lsn()` is `pub` (not dead-code-eligible), untouched.
- NOT verified (builds/tests/clippy forbidden this round): actual `cargo` compilation, the two new `compile_fail` doctests actually failing to compile in CI, and fmt output. Python-rewritten system_store.rs is CRLF on disk (matches the repo's autocrlf checkout; `git diff --numstat` shows 25/61, no EOL churn).

## Exhaustive audit (Round 8)

Read-only audit of the whole PR: every file in the union of
`git diff --name-only origin/master...HEAD` (**213** committed files) + every
`git git status --porcelain` entry (9 dirty + the untracked brief itself) =
**215 distinct real files** (resolve.rs is dirty-only; the brief carries both
tags). No builds, tests, clippy, fmt, scratch files, or git-mutating commands
were run. The ONLY write is this section.

### Removed-assertion scan (whole diff, cross-check)

Every removed `assert*`/`debug_assert*` line in the committed diff extracted
and matched: 22 removals (4×`remove_table("orders")`, 3×`remove_table("a")`,
3×`debug_assert!(false, "interner_cell must be populated…")`, 2×
`remove_table("users")`, 1×`users.counter()==0`, 1×`sorted_indexes…len()==2`,
1×`assert_coherent`, 1×`std::ptr::eq(ctx1.table(),…)`, 1×
`remove_table("t")`, 1×`remove_table("alpha")`, 1×`reader_gate().enter()`,
1×`wal.recover().len()==1`, 1×changefeed OnceCell comment-assert, 1×
`rows.counter()==2`, 1×`empty.counter()==0`) — identical set to Round 7's
scan; each is an accessor substitution (`ctx.table()`→`table_for_test`,
read-view accessors), a documented strengthening
(`wal.recover().len()==1` → `== inflight_before` + explicit
`assert_eq!(inflight_before.len(), 1)`), or removed together with the
`interner_cell` it guarded. 166 added assert lines. The Round-8 WORKING diff
removes ZERO assertion lines (scan run separately: only doc/comment/import
and the resolver-body line). No assertion deleted or weakened.

### Round 8 target verification (hunk-precise, from current source)

- **D1** ✓
  - `shadow_log.rs`: `encode_entry` (:52) `#[cfg(test)]` + Round-8 doc;
    `append` (:141) `#[cfg(test)]` + doc citing `0c59f555` and master's
    zero non-test callers; `append_batch` (:156) `#[cfg(test)]`; `entry_key`
    (:225) `#[cfg(test)]`. All four callers of `entry_key`/`encode_entry`
    are inside the cfg(test) `append`/`append_batch` bodies only;
    `read_from` (:172) builds keys via `ShadowKey::to_bytes` directly —
    **no non-test code references any gated item** (grep verified; the
    earlier grep hit at :164-165 is inside cfg(test) `append_batch`).
  - Engine test callers resolve: `migration/tests/coordinator_tests.rs`
    (:60,:78,:186,:235,:486,:495,:501,:582,:637,:647),
    `cutover_guard_tests.rs` (:63,:83,:136), `shadow_log_tests.rs`
    (throughout) — all under `#[cfg(test)]` mods, so they still see the
    gated helpers. Zero non-test callers of `.append(`/`.append_batch(`
    on `MigrationShadowLog` anywhere in the workspace.
  - `shadow_log()` deleted (coordinator.rs — hunk removes the whole
    `pub(crate) fn shadow_log()` incl. doc). Grep `shadow_log()` across
    crates: zero call sites (only `drain_shadow_log()` substring hits).
  - OBSOLETE verdict evidence chain spot-checked: `git show 0c59f555`
    message read in full — "Decision of the design review: do not build
    write interception … The source is quiesced instead; a write answers
    the typed error `table_migrating`", source frozen BEFORE interner
    replication. Master grep: zero production `MigrationShadowLog`
    `.append(`/`.append_batch(` callers (only `::new` admin_migration.rs:172
    / `::recover` :302 and the drain chain); master `.shadow_log()` call
    sites: zero. Live read/drain/purge chain intact on branch:
    `drain_shadow_log` (coordinator.rs:429) → `read_from`
    (shadow_log.rs:172) → `apply_shadow_entries` (:392); `purge` (:198);
    `recover` (:102); `current_lsn`/`shadow_lag` wired
    (coordinator.rs:335,:451-480); test coverage via coordinator_tests +
    `migration_index2.rs:439`.
- **D2** ✓ `resolve.rs:3-8`: imports now `query_value_to_inner_with` +
  `shamir_types::codecs::CodecError` (re-export confirmed at
  `shamir-types/src/codecs/mod.rs:9` — `pub use error::CodecError`;
  `codecs::error` module itself is private, so the root path is the
  correct one). `:710-717`: body is
  `query_value_to_inner_with(&qv, &|name| ctx.interner.get_ind(name)
  .ok_or_else(|| CodecError::Decode(format!("field name not interned:
  {name}")))).ok()` — matches the real signature
  (`codec.rs:32`: `F: Fn(&str) -> Result<InternerKey, CodecError>`;
  `Interner::get_ind` returns `Option<InternerKey>`, interner.rs:287). No
  `touch_ind`, no new ids, fails closed.
  - **No other public-reachable interning through a context-borrowed
    interner** — every named site checked:
    - `aggregate.rs:1167/:1206` (`query_value_to_inner` + `touch_ind`) —
      inside `step_aggregate_input` (`fn`, private, :1159), reached only
      from `build_aggregate_object` (`pub(super)`, :1227) whose interner
      comes from the engine scan chain (`apply_aggregate_all_with_context`
      is `pub(crate)`, :1770), NOT from a `FilterContext` field.
    - `exec.rs:194` (`touch_ind`) — inside `pub(crate) fn
      pre_intern_select_keys` (:163), sole caller
      `aggregate.rs:1697` (engine-internal HAVING path, engine-owned
      interner).
    - `read_index_scan.rs:138` (`touch_ind`) — `pub(crate)` scan impl
      taking `&Interner` from the engine-owned TableManager chain
      (read_index_scan.rs:39,502,641), never `ctx.interner`.
    - `admitted_ops.rs:205` (`query_value_to_inner`) — inside
      `intern_query_value_admitted`, permit-checked (admitted write path,
      by design).
    - All other `touch_ind`/`touch_with_id` hits are write/admitted/
      replication/recovery/seed paths on engine-owned interners
      (write_exec, table_manager_replication, sorted_index, index_mgmt,
      tx/recovery, drainer, apply_replicated, seed writer) or `cfg(test)`
      fixtures. read_only_views.rs docs updated truthfully (:13-17,:71-73).
- **D3** ✓ `table_manager.rs` doc now has SIX ` ```compile_fail ` blocks
  (:60,:68,:76,:84 original four — `info_store`, `data_store`, `interner`,
  `validator_registry` — intact; :96 `ctx.interner.touch_ind("x")` and
  :107 `view.touch_ind("x")` new). Fence syntax valid (all `///`-prefixed,
  open/close pairs matched, one example per block). First new block fails
  because `interner` is `pub(crate)`; second because
  `InternerReadView` has no `touch_ind` (grep: its methods are
  `get_ind`/`get_str`/`generation`/decode accessors only — read_only_views).
- **D4** ✓ `system_store.rs` = **1162 lines** (`wc -l`) ≤ 1181.
  Exactly ONE `persist_table_interner(&self, table: &TableManager,
  intern: Option<&QueryValue>)` (:164, `pub(crate)`), ONE admission
  (:168-175), `if let Some(record) = intern` →
  `intern_query_value_admitted` (:175-177), then
  `persist_interner_admitted`. Call-site census (grep, 20 total):
  **13 in-file** — :249 `Some(&rec)` (save_database: the former `to_inner`
  WRITE-prep site — interning genuinely needed ✓), :911 `Some(record)`
  (save_database_meta: former `to_inner` site ✓), :312,:434,:504,:575,
  :679,:940,:973,:997,:1063,:1088,:1158 `None` — each verified in the diff
  as a former plain `persist_table_interner(&table)` with NO adjacent
  `to_inner` (Round-7 grep `\.to_inner(` had exactly the two removed);
  **5 admin_replication.rs** — :106,:232,:351,:535,:810 all `None`
  (former plain sites, none with to_inner);
  **2 catalogue.rs** — :92,:132 `None` (former plain sites; :132 is the
  Round-7 L5 `self.system_store()` site, semantics preserved).
  `persist_table_interner_interning`: zero references anywhere
  (grep: only the two UPDATED doc mentions of `persist_table_interner`
  remain, at admitted_ops.rs:195 and read_only_views.rs:73).
- **Compile-readiness by reading** ✓ resolve.rs: `inner_value_to_query_value`,
  `Interner`, `InternerKey` imports still used elsewhere in the file;
  `CodecError` path resolves; closure arity/types match `F`. No dangling
  `query_value_to_inner` use in resolve.rs (import removed). cfg(test)
  items cannot trip `-D dead_code` (removed from non-test builds; used in
  test builds). `shadow_log()` deletion leaves no unused field
  (`self.shadow_log` still read by :335,:441,:456,:513,:554). Doc-comment
  code refs (`append` inside doc text) are not dead-code-eligible.
  `system_store.rs` diff is call-site arity only (2 args everywhere) — no
  struct-literal or Send-across-`.await` change (the merged fn holds the
  owned `RepoWritePermit` across awaits, same as Round 7's twin; Send).
  No new `pub` fn returning a `pub(crate)` type introduced this round
  (the merged fn is `pub(crate)` in a `pub(crate)`-module context already
  used by Round 7 without private_interfaces). The 2 new compile_fail
  doctests are additive to an existing gated doc — no doctest-count drift.
  CRLF on system_store.rs matches the repo checkout (git warns, numstat
  25/61 — no EOL churn).
- **Round-6/7 carried P2 debt — re-checked, ALL STILL OPEN (not fixed, per
  brief)**: `set_validator_registry` still `pub`
  (table_manager_validators.rs:107); `DbInstance::add_repo_with_durability`
  (:58), `remove_repo` (:161), `rename_repo` (:176) still pub and
  permit-free; `RepoInstance::repo_interner` still `pub async fn`
  (repo_instance.rs:810); `InternerManager::interner_cell` still `pub`
  (interner_manager.rs:137).

### Admit-site table (Round 8 deltas on top of Round 7)

| Entry | Chain | Permit |
|---|---|---|
| `SystemStore::persist_table_interner(table, Some(rec))` — save_database :249, save_database_meta :911 | ONE admit on SYSTEM repo → `intern_query_value_admitted` → `persist_interner_admitted` (same permit) | ✓ single admit |
| `SystemStore::persist_table_interner(table, None)` — 11 in-file + 5 admin_replication + 2 catalogue sites | ONE admit → `persist_interner_admitted` | ✓ single admit |
| `intern_query_value_admitted` (admitted_ops.rs:196) | `debug_check_permit` + `query_value_to_inner` | ✓ permit-held |
| `resolve_filter_value` (resolve.rs:707) | `get_ind` only — no admission needed, no mutation | n/a (pure) |
| MigrationShadowLog read/drain/purge/recover | reads + engine-internal purge; writes are cfg(test)-only (obsolete-by-design, 0c59f555) | read path, n/a |

No permit-free mutation is reachable from outside shamir-engine: the only
Round-8 mutation removal (`resolve_filter_value`'s `touch_ind`) is closed;
all 20 `persist_table_interner` sites admit; identity checks remain
debug-only (carried F7-3).

### Findings

- **F8-1 (P3, cosmetic)** — `shadow_log.rs:141-151`: `append` remains
  `pub(crate)` under `#[cfg(test)]`; `pub(crate)` is redundant for a
  test-only item (private would suffice). No lint fires; tidy-up candidate.
- **F8-2 (P3, observation)** — `table_manager.rs:96-102`: the first new
  compile_fail block would ALSO fail today for a second reason
  (`Interner::touch_ind` visibility is fine, but `ctx.interner` is
  `pub(crate)` — that is the intended single cause). No action; note only
  that the block's failure is not atomically attributable to one cause the
  way the four Round-7 blocks are.
- **F8-3 (carried, P2, open)** — Round-6 items #1-#3 (verified above) and
  Round-7 F7-1 (duplicated `debug_check_permit`,
  write_domain_index_core.rs) and F7-3 (debug-only identity checks in
  release) — all unchanged, all deliberately not fixed this round.
- No new P1/P2 findings.

### Per-file inventory (all 215 files, individually)

Legend: **[8]** = Round-8 working diff read hunk-by-hunk in full this
round; **[7]** = Round-7 dirty diff (read Round 7, re-verified by grep
this round); **[F]** = full committed diff read (this or Round-7 audit,
re-verified); **[S]** = symmetric test/bench/doc diff — this round covered
by (i) the exhaustive removed-assertion scan re-run over the whole
committed diff (every removed `assert*` line matched, above), (ii) the
Round-7 audit's targeted greps re-run (`.to_inner(`, `.shadow_log()`,
`.table()`, `pub async fn phase_*`, cross-crate raw getters — all zero),
(iii) Round-6 full reads; **[OK]** = no finding; **[F8-n]/[F7-n]** = finding.

1. `.github/workflows/ci.yml` — [7][F] L7 env mapping + doctest gate. OK
2. `crates/shamir-db/Cargo.toml` — [S] test-util feature chain. OK
3. `crates/shamir-db/src/shamir_db/execute/admin_describe.rs` — [S] read-view rewiring. OK
4. `crates/shamir-db/src/shamir_db/execute/admin_list.rs` — [S] read-view rewiring. OK
5. `crates/shamir-db/src/shamir_db/execute/admin_migration.rs` — [F] admit_pair / freeze-before-interner-copy / admitted twins. OK
6. `crates/shamir-db/src/shamir_db/execute/admin_replication.rs` — [8] 5 sites → `(&table, None)` (:106,:232,:351,:535,:810); all former plain-persist sites. OK
7. `crates/shamir-db/src/shamir_db/execute/admin_schema.rs` — [S] single `schema_repo_permit` per DDL op. OK
8. `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs` — [S] admitted status writes. OK
9. `crates/shamir-db/src/shamir_db/execute/helpers.rs` — [S] helper rewiring. OK
10. `crates/shamir-db/src/shamir_db/execute/replication_seed/catalogue.rs` — [8] 2 sites → `(&table, None)` (:92,:132); :132 keeps `self.system_store()` (L5). OK
11. `crates/shamir-db/src/shamir_db/execute/repo_admin/admin_schema_admission.rs` — [S] helpers moved verbatim. OK
12. `crates/shamir-db/src/shamir_db/shamir_db/changelog.rs` — [S] `InternerReadView` decode path. OK
13. `crates/shamir-db/src/shamir_db/shamir_db/core.rs` — [S] `_checked` create/drop wiring. OK
14. `crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs` — [S] read-view rewiring. OK
15. `crates/shamir-db/src/shamir_db/shamir_db/seed/subscriptions.rs` — [S] read-view rewiring. OK
16. `crates/shamir-db/src/shamir_db/shamir_db/seed_catalogue.rs` — [S] read-view rewiring. OK
17. `crates/shamir-db/src/shamir_db/shamir_db/table_management.rs` — [S] `get_table_admitted` facade; carried P2 #1 wiring. OK
18. `crates/shamir-db/src/shamir_db/shamir_db/tests/keyset_safe_write_barrier_tests.rs` — [S] fixture accessors. OK
19. `crates/shamir-db/src/shamir_db/shamir_db/tests/schema_rollback_tests.rs` — [S] +admission test. OK
20. `crates/shamir-db/src/shamir_db/system_store.rs` — [8] merged single `persist_table_interner(table, Option<&QueryValue>)`; 13 sites Some/None; banner rails stripped; 1162 lines. OK
21. `crates/shamir-db/src/shamir_db/tests/clean_horizon_tests.rs` — [S] fixture accessors. OK
22. `crates/shamir-db/src/shamir_db/tests/p1065_ddl_status_contract_tests.rs` — [S] read-view op-status. OK
23. `crates/shamir-db/src/shamir_db/tests/p1067_ddl_family_classification_tests.rs` — [S] read-view op-status. OK
24. `crates/shamir-db/src/shamir_db/tests/r0c_cross_family_drop_rename_tests.rs` — [S] read-view/permit. OK
25. `crates/shamir-db/src/shamir_db/tests/replication_ddl_tests.rs` — [S] permit threading. OK
26. `crates/shamir-db/src/shamir_db/tests/shutdown_quiesce_tests.rs` — [S] fixture accessors. OK
27. `crates/shamir-db/tests/ddl_wire_e2e/admission_barrier_tests.rs` — [S] permit/e2e. OK
28. `crates/shamir-db/tests/ddl_wire_e2e/drop_index_index2_support.rs` — [S] accessor. OK
29. `crates/shamir-db/tests/ddl_wire_e2e/drop_index_unified_resolution.rs` — [S] accessor. OK
30. `crates/shamir-db/tests/migration_index2.rs` — [7] L6 permit-lifetime fix (re-grepped: no permit held across self-admit). OK
31. `crates/shamir-db/tests/migration_source_freeze_tests.rs` — [S] zero permit usage. OK
32. `crates/shamir-db/tests/query_index2.rs` — [S] permit threading. OK
33. `crates/shamir-db/tests/retention_ddl.rs` — [S] +58 additive test. OK
34. `crates/shamir-db/tests/validators_lifecycle.rs` — [S] accessor. OK
35. `crates/shamir-engine/benches/batch_stage_parallelism.rs` — [S] test-util/accessors. OK
36. `crates/shamir-engine/benches/cursor_pages_depth.rs` — [S] OK
37. `crates/shamir-engine/benches/fk_cascade_index.rs` — [S] OK
38. `crates/shamir-engine/benches/for_each_eval.rs` — [S] OK
39. `crates/shamir-engine/benches/fts_indexed.rs` — [S] OK
40. `crates/shamir-engine/benches/keyset_seek.rs` — [S] OK
41. `crates/shamir-engine/benches/p1107_stale_value_gate.rs` — [S] OK
42. `crates/shamir-engine/benches/p1108_released_unique_growth.rs` — [S] OK
43. `crates/shamir-engine/benches/read_path_matrix.rs` — [S] OK
44. `crates/shamir-engine/benches/select_pipeline.rs` — [S] OK
45. `crates/shamir-engine/benches/streaming_topk.rs` — [S] OK
46. `crates/shamir-engine/benches/tx_aware_reads.rs` — [S] OK
47. `crates/shamir-engine/benches/tx_pipeline.rs` — [S] OK
48. `crates/shamir-engine/benches/when_skip_eval.rs` — [S] OK
49. `crates/shamir-engine/Cargo.toml` — [S] doctest=true + test-util. OK
50. `crates/shamir-engine/src/db_instance/db_instance.rs` — [F] checked twins; **add/remove/rename still open (P2 #2)**. OK-with-carried-P2
51. `crates/shamir-engine/src/db_instance/tests/mod.rs` — [S] mod wiring. OK
52. `crates/shamir-engine/src/db_instance/tests/write_domain_membership_tests.rs` — [S] additive tests. OK
53. `crates/shamir-engine/src/meta/tests/lifecycle/lifecycle_record_tests.rs` — [S] accessor. OK
54. `crates/shamir-engine/src/migration/coordinator.rs` — [8] `shadow_log()` deleted (hunk-verified, zero dangling refs); drain/purge/recover chain intact. OK
55. `crates/shamir-engine/src/migration/shadow_log.rs` — [8] `encode_entry`/`append`/`append_batch`/`entry_key` → `#[cfg(test)]` + evidence docs; all remaining callers cfg(test); read_from independent of gated helpers. F8-1 (cosmetic)
56. `crates/shamir-engine/src/repo/changelog_store.rs` — [S] module wiring. OK
57. `crates/shamir-engine/src/repo/changelog_store/repo_membership_impl.rs` — [F] get_table self-admits; add/remove permit-gated. OK
58. `crates/shamir-engine/src/repo/changelog_store/tests/mod.rs` — [S] mod wiring. OK
59. `crates/shamir-engine/src/repo/changelog_store/tests/write_domain_tests.rs` — [S] additive tests. OK
60. `crates/shamir-engine/src/repo/changelog_store/write_domain.rs` — [F] domain/permit/admit_pair; unforgeable-permit fence. OK
61. `crates/shamir-engine/src/repo/changelog_store/write_domain_membership.rs` — [F] admitted membership twins. OK
62. `crates/shamir-engine/src/repo/durability_policy/durability_policy/repo_impl.rs` — [S] factory-open admits once. OK
63. `crates/shamir-engine/src/repo/mod.rs` — [S] re-exports. OK
64. `crates/shamir-engine/src/repo/repo_instance.rs` — [S] rename twins; **`repo_interner` still pub (P2 #3)**. OK-with-carried-P2
65. `crates/shamir-engine/src/repo/seed_install/writer.rs` — [S] seed install admits once. OK
66. `crates/shamir-engine/src/repo/tests/per_table_mvcc_token_names_deadlock_tests.rs` — [S] additive. OK
67. `crates/shamir-engine/src/repo/tests/repo_instance_tests.rs` — [S] accessors. OK
68. `crates/shamir-engine/src/repo/tests/repo_tx_accessors_tests.rs` — [S] accessors. OK
69. `crates/shamir-engine/src/repo/tests/table_init_retry_tests.rs` — [S] Notify adaptation. OK
70. `crates/shamir-engine/src/query/filter/eval_context.rs` — [7] interner field `pub(crate)`. OK
71. `crates/shamir-engine/src/query/filter/resolve.rs` — [8] D2 read-only resolver (imports, doc, body — all read hunk-by-hunk; closure matches signature). OK
72. `crates/shamir-engine/src/table/doctor.rs` — [S] repair self-admits; repair_admitted single-admission. OK
73. `crates/shamir-engine/src/table/mod.rs` — [7] L4 re-export. OK
74. `crates/shamir-engine/src/table/record_counter.rs` — [S] counter admits. OK
75. `crates/shamir-engine/src/table/table_manager_buffer.rs` — [S] buffer-config self-admits. OK
76. `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` — [F] split body; `PhaseBAResult` pub(crate) here. OK
77. `crates/shamir-engine/src/table/table_manager_replication.rs` — [S] replicate/bulk-populate self-admits. OK
78. `crates/shamir-engine/src/table/table_manager.rs` — [8] two new compile_fail blocks added on the gate doc; four originals intact; fences valid. OK
79. `crates/shamir-engine/src/table/table_manager_sorted_index.rs` — [S] sorted create/drop self-admits. OK
80. `crates/shamir-engine/src/table/table_manager_validators.rs` — [S] bindings self-admit; **set_validator_registry still pub (P2 #1)**. OK-with-carried-P2
81. `crates/shamir-engine/src/table/write_receipt/admission/admitted_ops.rs` — [8] stale doc ref to `persist_table_interner_interning` renamed to `persist_table_interner` with `Some(record)` (:192-196); fn body untouched. OK
82. `crates/shamir-engine/src/table/write_receipt/admission/read_only_views.rs` — [8] :13-17 Round-8 truthful claim; :73 stale doc ref fixed. OK
83. `crates/shamir-engine/src/table/write_receipt/admission/write_domain_barrier.rs` — [7] L1.3 borrows; carried F7-2 stale comment. F7-2
84. `crates/shamir-engine/src/table/write_receipt/admission/write_domain_index_admitted.rs` — [F] create/drop sorted + status plumbing. OK
85. `crates/shamir-engine/src/table/write_receipt/admission/write_domain_index_core.rs` — [F] admitted twins; carried F7-1 duplicate debug_check. F7-1
86. `crates/shamir-engine/src/table/write_receipt/admission/write_domain_online_index.rs` — [7] pub(crate) phase fns. OK
87. `crates/shamir-engine/src/table/write_receipt/admission/write_domain_rename_admitted.rs` — [F] recovery + rename admitted swaps. OK
88. `crates/shamir-engine/src/table/tests/batch_hoist_invariants_tests.rs` — [S] OK
89. `crates/shamir-engine/src/table/tests/doctor_tests.rs` — [S] OK
90. `crates/shamir-engine/src/table/tests/f50_index_lifecycle_spike_tests.rs` — [S] OK
91. `crates/shamir-engine/src/table/tests/f50_step2_index_lifecycle_tests.rs` — [S] OK
92. `crates/shamir-engine/src/table/tests/f57_index_create_barrier_tests.rs` — [S] OK
93. `crates/shamir-engine/src/table/tests/f69_write_barrier_single_atomic_tests.rs` — [S] OK
94. `crates/shamir-engine/src/table/tests/f70_lock_order_inversion_tests.rs` — [S] OK
95. `crates/shamir-engine/src/table/tests/f72_planner_invisibility_tests.rs` — [S] OK
96. `crates/shamir-engine/src/table/tests/f76_drop_visibility_tests.rs` — [S] OK
97. `crates/shamir-engine/src/table/tests/f95_ddl_admission_tests.rs` — [S] OK
98. `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` — [S] OK
99. `crates/shamir-engine/src/table/tests/index2_create_barrier_tests.rs` — [S] OK
100. `crates/shamir-engine/src/table/tests/index2_empty_result_tests.rs` — [S] OK
101. `crates/shamir-engine/src/table/tests/index_build_explicit_pin_tests.rs` — [S] OK
102. `crates/shamir-engine/src/table/tests/index_rename_concurrency_tests.rs` — [S] OK
103. `crates/shamir-engine/src/table/tests/migration_freeze_tests.rs` — [S] OK
104. `crates/shamir-engine/src/table/tests/non_tx_ack_wait_tests.rs` — [S] OK
105. `crates/shamir-engine/src/table/tests/p02_base_index_rederive_tests.rs` — [S] OK
106. `crates/shamir-engine/src/table/tests/p1008_instance_provenance_tests.rs` — [S] OK
107. `crates/shamir-engine/src/table/tests/p1011_reader_drain_tests.rs` — [S] OK
108. `crates/shamir-engine/src/table/tests/p1037_sorted_reader_drain_tests.rs` — [S] OK
109. `crates/shamir-engine/src/table/tests/p1038_index2_reader_drain_tests.rs` — [S] OK
110. `crates/shamir-engine/src/table/tests/p1060_online_index_crash_recovery_tests.rs` — [S] OK
111. `crates/shamir-engine/src/table/tests/p1061_pipeline_property_tests.rs` — [S] OK
112. `crates/shamir-engine/src/table/tests/p1063_multi_fts_index_stats_tests.rs` — [S] OK
113. `crates/shamir-engine/src/table/tests/p1087_phase_b_a_tests.rs` — [S] OK
114. `crates/shamir-engine/src/table/tests/p1088_phase_c_d_tests.rs` — [S] OK
115. `crates/shamir-engine/src/table/tests/p1098_gen_read_order_tests.rs` — [S] OK
116. `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` — [S] OK
117. `crates/shamir-engine/src/table/tests/r0a_registry_watermark_admission_tests.rs` — [S] OK
118. `crates/shamir-engine/src/table/tests/read_one_tx_tests.rs` — [S] OK
119. `crates/shamir-engine/src/table/tests/schema_activation_barrier_tests.rs` — [S] OK
120. `crates/shamir-engine/src/table/tests/snapshot_vector_tests.rs` — [S] OK
121. `crates/shamir-engine/src/table/tests/stream_tx_tests.rs` — [S] OK
122. `crates/shamir-engine/src/table/tests/table_manager_tests.rs` — [7] `table_for_test` rewiring. OK
123. `crates/shamir-engine/src/table/tests/tx_read_shortcut_tests.rs` — [S] OK
124. `crates/shamir-engine/src/table/tests/tx_scan_overlay_tests.rs` — [S] OK
125. `crates/shamir-engine/src/table/tests/version_cas_tests.rs` — [S] OK
126. `crates/shamir-engine/src/table/tests/write_receipt/crud_tests.rs` — [S] OK
127. `crates/shamir-engine/src/table/tests/write_receipt/mod.rs` — [S] OK
128. `crates/shamir-engine/src/table/tests/write_receipt/write_domain_admission_matrix_tests.rs` — [S] additive. OK
129. `crates/shamir-engine/src/table/tests/write_receipt/write_domain_barrier_tests.rs` — [S] additive. OK
130. `crates/shamir-engine/src/table/tests/write_receipt/write_domain_counter_tests.rs` — [S] additive. OK
131. `crates/shamir-engine/src/table/tests/write_receipt/write_domain_shared.rs` — [S] fixture helper. OK
132. `crates/shamir-engine/src/tx/drainer.rs` — [S] permit-free leader path (by design). OK
133. `crates/shamir-engine/src/tx/orphan_repair/tests/repair_tests.rs` — [S] accessors. OK
134. `crates/shamir-engine/src/tx/recovery.rs` — [S] replay/recovery admits. OK
135. `crates/shamir-engine/src/tx/tests/a8_interner_delta_tests.rs` — [S] OK
136. `crates/shamir-engine/src/tx/tests/acceptance_tests.rs` — [S] OK
137. `crates/shamir-engine/src/tx/tests/backpressure_gc_tests.rs` — [S] OK
138. `crates/shamir-engine/src/tx/tests/base_index_tx_tests.rs` — [S] OK
139. `crates/shamir-engine/src/tx/tests/commit_async_visibility_tests.rs` — [S] OK
140. `crates/shamir-engine/src/tx/tests/commit_phase5_defer_tests.rs` — [S] OK
141. `crates/shamir-engine/src/tx/tests/commit_phase5_tests.rs` — [S] OK
142. `crates/shamir-engine/src/tx/tests/commit_tests.rs` — [S] OK
143. `crates/shamir-engine/src/tx/tests/cutover_tests.rs` — [S] OK
144. `crates/shamir-engine/src/tx/tests/drainer_tests.rs` — [S] OK
145. `crates/shamir-engine/src/tx/tests/f73_rederive_fail_closed_tests.rs` — [S] OK
146. `crates/shamir-engine/src/tx/tests/f74_bump_apply_ordering_tests.rs` — [S] OK
147. `crates/shamir-engine/src/tx/tests/gc_tests.rs` — [S] OK
148. `crates/shamir-engine/src/tx/tests/index_rollback_tests.rs` — [S] OK
149. `crates/shamir-engine/src/tx/tests/interner_checkpoint_tests.rs` — [S] OK
150. `crates/shamir-engine/src/tx/tests/journal_publish_order_tests.rs` — [S] OK
151. `crates/shamir-engine/src/tx/tests/non_tx_mvcc_routing_tests.rs` — [S] OK
152. `crates/shamir-engine/src/tx/tests/oracle_stress_tests.rs` — [S] OK
153. `crates/shamir-engine/src/tx/tests/p1096_tx_aware_unique_check_tests.rs` — [S] OK
154. `crates/shamir-engine/src/tx/tests/p1097_remove_posting_owner_tests.rs` — [S] OK
155. `crates/shamir-engine/src/tx/tests/p1100_stale_snapshot_delete_posting_tests.rs` — [S] OK
156. `crates/shamir-engine/src/tx/tests/p1101_released_skip_durable_check_tests.rs` — [S] OK
157. `crates/shamir-engine/src/tx/tests/pessimistic_lock_tests.rs` — [S] OK
158. `crates/shamir-engine/src/tx/tests/pessimistic_lost_update_tests.rs` — [S] OK
159. `crates/shamir-engine/src/tx/tests/post_wal_orphan_replay_tests.rs` — [S] OK
160. `crates/shamir-engine/src/tx/tests/read_your_own_writes_tests.rs` — [S] OK
161. `crates/shamir-engine/src/tx/tests/recovery_tests.rs` — [S] strengthened assertions. OK
162. `crates/shamir-engine/src/tx/tests/rederive_stale_value_dedup_cache_tests.rs` — [S] OK
163. `crates/shamir-engine/src/tx/tests/repo_diagnostics_tests.rs` — [S] OK
164. `crates/shamir-engine/src/tx/tests/snapshot_stress_tests.rs` — [S] OK
165. `crates/shamir-engine/src/tx/tests/snapshot_visibility_park_tests.rs` — [S] OK
166. `crates/shamir-engine/src/tx/tests/ssi_phantom_tests/gc_tests.rs` — [S] OK
167. `crates/shamir-engine/src/tx/tests/ssi_phantom_tests/integration_tests.rs` — [S] OK
168. `crates/shamir-engine/src/tx/tests/ssi_phantom_tests/mvcc1_fix_tests.rs` — [S] OK
169. `crates/shamir-engine/src/tx/tests/ssi_phantom_tests/perf233_tests.rs` — [S] OK
170. `crates/shamir-engine/src/tx/tests/ssi_phantom_tests/phase_2bis_tests.rs` — [S] OK
171. `crates/shamir-engine/src/tx/tests/ssi_phantom_tests/predicate_capture_tests.rs` — [S] OK
172. `crates/shamir-engine/src/tx/tests/ssi_phantom_tests/update_phantom_tests.rs` — [S] OK
173. `crates/shamir-engine/src/tx/tests/ssi_stress_tests.rs` — [S] OK
174. `crates/shamir-engine/src/tx/tests/ssi_unique_serialization_tests.rs` — [S] OK
175. `crates/shamir-engine/src/tx/tests/truncation_tests.rs` — [S] OK
176. `crates/shamir-engine/src/tx/tests/tx_commit_writer_drain_tests.rs` — [S] OK
177. `crates/shamir-engine/src/tx/tests/tx_vector_delete_tests.rs` — [S] OK
178. `crates/shamir-engine/src/validator/persistence.rs` — [S] `save_validators_metadata` pub(crate). OK
179. `crates/shamir-server/Cargo.toml` — [S] test-util dev-dep. OK
180. `crates/shamir-server/src/db_handler/cursor_handlers.rs` — [S] engine-side `build_filter_context`. OK
181. `crates/shamir-server/src/doctor.rs` — [S] `create_table_checked`. OK
182. `crates/shamir-server/src/replication/seed/admission.rs` — [S] `filter_context` engine-side. OK
183. `crates/shamir-server/src/replication/seed/fence.rs` — [S] `filter_context` engine-side. OK
184. `crates/shamir-server/src/replication/seed/tests/loop_tests.rs` — [S] accessor. OK
185. `crates/shamir-server/src/replication/supervisor.rs` — [S] read-view rewiring. OK
186. `crates/shamir-server/src/replication/tests/follower_loop_tests.rs` — [S] accessor. OK
187. `crates/shamir-server/src/replication/tests/read_after_replication_tests.rs` — [S] accessor. OK
188. `crates/shamir-server/src/replication/tests/supervisor_tests.rs` — [S] accessor. OK
189. `crates/shamir-server/src/replication_seed/export/exporter.rs` — [S] read-view introspection. OK
190. `crates/shamir-server/src/replication_seed/install/subscription/tests/disk_tests.rs` — [S] OK
191. `crates/shamir-server/src/replication_seed/install/tests/disk_tests.rs` — [S] OK
192. `crates/shamir-server/src/replication_seed/tests/artifact_tests.rs` — [S] OK
193. `crates/shamir-server/src/replication_seed/tests/bounds_tests.rs` — [S] OK
194. `crates/shamir-server/src/replication_seed/tests/disk_export_tests.rs` — [S] OK
195. `crates/shamir-server/src/server/server_launcher.rs` — [S] `create_table_checked`. OK
196. `crates/shamir-server/src/subscriptions/bridge.rs` — [S] `Arc<InternerReadView>`. OK
197. `crates/shamir-server/src/subscriptions/decode_cache.rs` — [S] `InternerReadView` cache type. OK
198. `crates/shamir-server/src/subscriptions/filter_eval.rs` — [S] eval via read view. OK
199. `crates/shamir-server/src/subscriptions/target_match.rs` — [S] read-view rewiring. OK
200. `crates/shamir-server/src/subscriptions/tests/filter_eval_tests.rs` — [S] OK
201. `crates/shamir-server/src/subscriptions/tests/filter_lens_parity_tests.rs` — [S] OK
202. `crates/shamir-server/src/subscriptions/tests/target_match_tests.rs` — [S] OK
203. `crates/shamir-server/src/tests/shutdown_truth_tests.rs` — [S] accessor. OK
204. `crates/shamir-server/tests/db_handler.rs` — [S] accessor. OK
205. `crates/shamir-server/tests/doctor_e2e.rs` — [S] accessor. OK
206. `crates/shamir-server/tests/hmac_gate.rs` — [S] accessor. OK
207. `crates/shamir-server/tests/user_admin_port_wire.rs` — [S] accessor. OK
208. `docs/dev-artifacts/prompts/remediation/108-s5cx-repo-write-domain.md` — [S] docs. OK
209. `docs/dev-artifacts/prompts/remediation/109-s5cx-round3.md` — [S] docs. OK
210. `docs/dev-artifacts/prompts/remediation/110-s5cx-round4.md` — [S] docs. OK
211. `docs/dev-artifacts/prompts/remediation/111-s5cx-round5.md` — [S] docs. OK
212. `docs/dev-artifacts/prompts/remediation/112-s5cx-round6.md` — [S] docs. OK
213. `docs/dev-artifacts/prompts/remediation/113-s5cx-round7.md` — [F] Round-7 brief + inventory. OK
214. `docs/dev-artifacts/prompts/remediation/114-s5cx-round8.md` — [8] this brief (inventory above + this appended audit; untracked). OK
215. `docs/guide-docs/follower-journal-durability.md` — [S] docs. OK

### Coverage statement

**Inspected hunk-by-hunk this round, in full:** the ENTIRE Round-8 working
diff — all 9 dirty files, every hunk, every ADDED line (system_store.rs 86
changed lines incl. every Some/None call site and every stripped banner;
shadow_log.rs all four cfg gates + docs; coordinator.rs deletion; resolve.rs
imports + doc + body; table_manager.rs both new doctest blocks;
admitted_ops.rs + read_only_views.rs doc fixes; admin_replication.rs and
catalogue.rs every call-site hunk) — plus the untracked brief. Round-8
targets verified from current source (not from the diff) for D1 gating +
caller resolution, D2 signature/import resolution and the four named
interning sites, D3 fence enumeration, D4 line count and the full
20-site Some/None census. Whole-diff removed-assertion scan re-run
(22 removed / 166 added committed; 0 removed in the working diff).

**[S] symmetric test/bench/doc files (items 2-4,6-9,11-19,21-34,35-48
(except as tagged),49,51-53,56-59,62-69,72-79 (except as tagged),88-207):
their ADDED lines were covered by (i) the exhaustive removed-assertion
scan re-run this round, (ii) the Round-7 audit's targeted greps re-run
this round (`.to_inner(`, `.shadow_log()`, `.table()`, `pub async fn
phase_*`, cross-crate raw interner/registry getters — all zero), and
(iii) Round-6/7 full reads — NOT by re-reading each added line again this
round; none of them is touched by the Round-8 diff.** Every production
file, every Round-8 target, both manifests' relevant entries, CI, and all
docs are covered; the coverage gap is limited to that carried-over
symmetric-class re-read, identical to the class the Round-7 audit
declared. No builds, tests, clippy, doctests, fmt, or git mutations were
run; compile-readiness was verified by reading only.

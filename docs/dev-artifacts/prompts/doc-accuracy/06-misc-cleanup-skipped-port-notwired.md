# Documentation Accuracy 6g — TS `skipped` field, stale port 13760, stale "not wired" comments

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Seventh (final) item of "Этап 6 — Documentation accuracy"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 09 (finding #6, #11) and report 05
(`docs/dev-artifacts/research/2026-07-17-release-audit/
05-incomplete-features-gaps.md`, its own "## Stale 'not wired / stub'
comments" section). Three unrelated small fixes, investigated below —
you may tackle them in any order, they don't interact.

### 1. TS `QueryResult` interface is missing the `skipped` field (small REAL CODE fix)

`docs/guide-docs/guide/01-queries.md` (lines ~397-399, ~459-472) shows a
canonical example reading `resp.results.debit.skipped` on the TypeScript
client. The Rust wire type genuinely has this field —
`crates/shamir-query-types/src/read/query_result.rs:90`:
```rust
/// Conditional-execution status (Epic03/B, #645): `true` when this
/// alias's op did NOT run — either its own `when` evaluated `false`, or
/// it was cascade-skipped because a `DataFlow`/`Both`-provenance
/// dependency was itself skipped. `false` (the default, omitted from
/// the wire) means the op executed normally; existing peers that don't
/// know this field never observe it.
#[serde(default, skip_serializing_if = "std::ops::Not::not")]
pub skipped: bool,
```
But `crates/shamir-client-ts/src/core/types/batch.ts`'s `QueryResult`
interface (~lines 199-210) does NOT declare it (confirmed via grep — zero
hits for `skipped` in `batch.ts`). A TypeScript user following the guide
literally gets a compile error (`Property 'skipped' does not exist on
type 'QueryResult'`). Note `edge_provenance` WAS added to the TS
`BatchResponse` type already (`batch.ts:258`) — `skipped` is the one field
that fell through the cracks; this is real client-type drift, not a
guide inaccuracy.

**Fix**: add `skipped?: boolean;` to the TS `QueryResult` interface in
`batch.ts`, mirroring the Rust field's semantics and the JSDoc style
already used for the adjacent `explain` field in the same interface
(default-omitted-when-false, so optional in the TS type is the correct
shape). This is a genuine TS SDK bug fix, not just a doc correction — treat
it as a small real code change with a regression test.

### 2. Stale example port `13760` (cosmetic, LOW — but appears in 4 places, not the 2 the work-plan names)

`docs/guide-docs/guide/00-quickstart.md:37` and `04-access.md:51` use
`port: 13760` in TypeScript connection examples — **confirmed via grep
there are actually 2 MORE occurrences beyond what the work-plan names**:
`docs/guide-docs/architecture/ARCHITECTURE.md:616` and `:932` also use
`port: 13760`. No default port exists anywhere in server code (listeners
are mandatory config, confirmed via `crates/shamir-server/src/
config.rs:29`'s own doc-comment example, which — like every other guide
doc — uses `7331`). `13760` appears nowhere in `crates/shamir-server/src`.

**Fix**: change all 4 occurrences of `13760` to `7331` (the port
consistently used as the canonical TCP example everywhere else:
`07-operations.md`, `IMPLEMENTATION_GUIDE.md`, `TRANSPORT_TCP.md`,
`config.rs`'s own doc comment) so a reader cross-referencing floors 0/4
against the operations guide and the protocol spec doesn't hit a
confusing mismatch.

### 3. Three stale "not wired" doc-comments in real Rust source (report 05's own tail section)

Report 05 has a dedicated section, "## Stale 'not wired / stub' comments
(doc rot — NOT gaps, do not re-fix the code)", citing THREE module-level
doc comments that claim a primitive is unwired/scaffold-only, when it is
in fact already wired into real call paths. **Confirmed by direct
inspection — all three still read exactly as report 05 describes:**

1. `crates/shamir-wal/src/wal_group_commit.rs` (~lines 65-66):
   > `//! PURELY ADDITIVE: not wired into `RepoWalManager` or the commit
   > path (that is W3/W4). Marked `#[allow(dead_code)]` while unwired.`

   **False today** — `crates/shamir-tx/src/repo_wal_manager.rs` confirms
   `RepoWalManager` wraps `Arc<WalGroupCommit>` (`group: Arc<WalGroupCommit>`
   field, doc comment "All writes funnel through `WalGroupCommit`") — it
   IS the funnel now, not an unwired scaffold. **Also check**: this file
   has `#[allow(dead_code)]` at (at least) two more sites (~lines 118, 153)
   that were presumably added when the module WAS unwired — since
   `RepoWalManager` now uses it, verify whether those specific
   `#[allow(dead_code)]` attributes are still needed (i.e., is the
   attributed item genuinely still unused, or is it reachable via
   `RepoWalManager` now and the attribute is dead weight). If clippy
   would flag the code as used without the attribute, remove it; if some
   specific method genuinely still has no caller, leave that one
   attribute but correct the surrounding prose. Don't blindly strip all
   `#[allow(dead_code)]` without checking each site individually.

2. `crates/shamir-types/src/record_view/mod.rs` (~lines 5-7):
   > `//! **ADDITIVE — not wired into the engine.** Stage 2 puts this
   > behind a `RecordRef` trait; Stages 3-4 migrate consumers.`

   **False today** — confirmed via `grep -rl "RecordView\|RecordRef"
   crates/shamir-engine/src/` → 34 files reference it; it's used across
   read_exec, filters, projections, crud, streaming per report 05's own
   citation. The migration (Stages 2-4 this comment describes as future)
   has clearly completed.

3. `crates/shamir-tx/src/versioned_overlay.rs` (~line 14):
   > `//! **Scaffold (P1a).** This module is additive — it is not wired
   > into any read or write path yet. P1b–P1e will integrate it.`

   **False today** — confirmed via grep: `VersionedOverlay` is referenced
   in `crates/shamir-tx/src/mvcc_store/mod.rs` (and has its own test file
   `crates/shamir-tx/src/tests/versioned_overlay_tests.rs`) — it's wired
   into the MVCC store's read/write path, not a dangling scaffold.

## The task

1. Add `skipped?: boolean;` to `QueryResult` in
   `crates/shamir-client-ts/src/core/types/batch.ts`, with a JSDoc comment
   mirroring the Rust field's doc comment (conditional-execution status,
   #645 cross-reference, default-omitted-when-false semantics). Add a
   regression test in the TS test suite (check
   `crates/shamir-client-ts/src/__tests__/` or `src/core/__tests__/` for
   where `QueryResult`/`BatchResponse` deserialization is already tested,
   mirror that pattern) proving a msgpack payload with `skipped: true`
   decodes into a `QueryResult` object exposing `.skipped === true`
   without a TypeScript compile error.
2. Change all 4 occurrences of `port: 13760` (or `port:13760` — check
   exact formatting) to `port: 7331` in `00-quickstart.md`, `04-access.md`,
   and BOTH occurrences in `ARCHITECTURE.md`.
3. Rewrite the three stale doc comments in `wal_group_commit.rs`,
   `record_view/mod.rs`, and `versioned_overlay.rs` to accurately describe
   the CURRENT wired-in state (mirror this campaign's tone from similar
   fixes — task 6a's `08-interconnect.md` rebalance is a good model: state
   plainly what's real, cite the actual consumer, don't just delete the
   history — a brief note that this module WAS built additively and is
   NOW the production path is fine). Investigate and resolve the
   `wal_group_commit.rs` `#[allow(dead_code)]` question per the
   instructions in section 3.1 above — report your finding either way.

## Out of scope

- Do NOT touch any other TS type beyond adding the one missing
  `skipped` field to `QueryResult` — don't do a broader TS/Rust wire-type
  parity audit, that's a much bigger task than this brief.
- Do NOT touch the OTHER findings in report 09 (#5, #7, #8, #10, #12,
  #13, #14) or report 05's other findings — this brief is scoped to
  exactly the three items above.
- Do NOT touch anything from the already-completed Этапы 1-5 or tasks
  6a-6f — this brief is scoped to these three fixes only.

## Verification (MANDATORY before you report done)

- For item 1 (TS `skipped` field): run the TypeScript test suite for
  `shamir-client-ts` (check `crates/shamir-client-ts/package.json` for
  the test script — likely `npm test` or `vitest`) and report the
  literal output; confirm the new field compiles and the new test
  passes and would have failed (type error or assertion failure) before
  your fix.
- For items 2 and 3 (docs-only): no `cargo test`/`clippy`/`fmt` gate
  applies to the markdown changes — state this explicitly. For item 3's
  Rust doc-comment changes (and the potential `#[allow(dead_code)]`
  removal), run `cargo fmt -p shamir-wal -p shamir-types -p shamir-tx --
  --check` and `cargo clippy --workspace --all-targets -- -D warnings`
  and `./scripts/test.sh -p shamir-wal -p shamir-types -p shamir-tx
  --full` to confirm no regression from touching real source files
  (even though only comments/attributes change, prove it with a real
  build+test run, don't assume comment-only edits are risk-free without
  checking).
- Report explicitly what you found/decided for the
  `wal_group_commit.rs` `#[allow(dead_code)]` question.
- Confirm the final count of `13760` occurrences in the repo is zero
  (`grep -rn "13760" docs/`).

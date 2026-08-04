# Brief — P1-2: CREATE/RENAME/DROP can return `Err` after a partial live mutation already persisted

Task: #967 in the session TaskList. Source: `docs/dev-artifacts/new-wave-readonly-review.md` §P1-2. Depends on the persisted-state-machine work from #959/#961/#962/#972 (durable tombstones + idempotent crash recovery) and #966 (doctor visibility) — all already landed. Read this brief in full; the scope here is deliberately narrower than the review's headline "unified DDL result contract" framing, for reasons explained below.

## What the review is actually pointing at — verified against current code

**The core mechanism is already correct and mostly in place.** Every
multi-phase DDL op this session already fixed (#959/#961/#962/#972) follows
the same pattern: persist a durable marker (a `Building` registration, or a
drop/rename tombstone) BEFORE the risky multi-step work, then either
complete it within the same call or — if a crash/restart intervenes —
resume it idempotently on next open (`recover_in_progress_drops`, sorted's
`recover_in_progress_renames`, etc.). This IS "a persisted state machine
with idempotent recovery" — one of the two acceptable fixes the review
itself names ("либо честный rollback, либо persisted state machine с
идемпотентным recovery").

**The concrete gap**: verified in
`crates/shamir-index/src/base_index/index_manager.rs::create_index_from_records`
(~line 899-935): Phase 1 durably persists the `Building` registration
(`self.save_index_info().await?`) — if Phase 2 (the backfill loop,
`self.info_store.set_many(posting_writes).await?`) THEN fails with a store
error, the function returns `Err(...)` to its caller, and that `Err`
propagates all the way to the CLIENT as a bare error message. But the
`Building` registration is ALREADY durably persisted at that point — the
client's error response gives ZERO indication that a partial mutation
happened, or how to check/resolve it (this exact scenario is precisely why
#966 had to add `doctor::verify()` visibility for stuck-Building indexes —
without it, an operator has no way to discover this state exists at all).

**This same shape** — persist-then-later-fallible-step, with the later
failure surfacing as an opaque `Err` — likely recurs across the other 3
CREATE index families (unique, sorted, index2) and possibly RENAME/DROP's
own multi-phase paths. Find every instance via the same pattern (a durable
persist call, e.g. `save_index_info`/similar, followed later in the SAME
function by a fallible operation using `?` that can return before the
op fully completes) in:
- `crates/shamir-index/src/base_index/index_manager.rs` (regular + unique
  CREATE — check `create_unique_index_from_records` too)
- `crates/shamir-index/src/base_index/sorted_index_manager.rs` (sorted
  CREATE)
- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs`
  (`create_index_v2`, index2 CREATE)
- DROP/RENAME paths for regular/unique/sorted/index2 (`#959`/`#972`/`#961`/
  `#962` already added tombstone+recovery for these — check whether their
  OWN in-flight-call error paths have the same "opaque Err, no context"
  gap, even though the CRASH-recovery half is already solid).

## The fix — additive error-message enrichment, NOT a wire-protocol redesign

**Do NOT attempt the review's "operation ID + status" unified DDL result
contract** in this pass — that requires changing the wire shape of every
DDL response across `shamir-query-types`, every handler in
`crates/shamir-db/src/shamir_db/execute/`, AND both client SDKs
(`shamir-client-ts`, `shamir-query-builder`). That is a large, invasive,
cross-cutting wire-protocol change deserving its own dedicated design task
— NOT appropriate for a single delegated pass. It will be tracked
separately (the orchestrator files this after your report, mirroring how
#966's readiness/metrics follow-up was tracked as #984 — you do not need
to create that task yourself).

**What TO do**: at each site you find matching the pattern above, when the
LATER fallible step fails after an EARLIER durable persist already
succeeded, enrich the returned error's message (still a plain
`DbError`/`BuilderError`-style string — no new type, no wire change) to
explicitly state:
1. that a partial/durable state change WAS persisted (name the specific
   thing — e.g. "index 'foo' was registered as Building"),
2. what the CURRENT actual state is likely to be (e.g. "backfill did not
   complete; the index is NOT queryable"),
3. how an operator/caller can check or resolve it (e.g. "call
   TableManager::verify() to confirm, or TableManager::repair() to
   rebuild it").

This is purely additive (richer error TEXT at existing failure points,
using the existing error types — `DbError::Internal(format!(...))` or
equivalent, no new variants needed unless a site genuinely has none
available) and requires no coordination with client SDKs or wire schemas.

## Required work

1. Grep/read each of the 4 CREATE-index-family paths (+ RENAME/DROP paths)
   for the persist-then-later-fallible-step shape. For each genuine
   instance found, enrich the error message per the 3 points above.
2. Do NOT touch the earlier, ALREADY-CORRECT persist step itself, and do
   NOT touch the crash-recovery paths (`recover_in_progress_drops`,
   `recover_in_progress_renames`, F-50 Step 3b index2 self-heal) — those
   are separately verified and out of scope.
3. Add/extend tests proving the enriched error message text appears when
   the later step is forced to fail. Reuse EXISTING deterministic failure
   injection if it exists (check for a fault-injection seam near each
   site — e.g. a test-only `Store` wrapper that can be told to fail the
   Nth call) rather than inventing a new one. If no such seam exists for a
   given site and building one would be substantial, note the gap in your
   report instead of improvising something fragile.

## Gate (MANDATORY — this is production code, not test-only)

Run for every crate you touch (likely `shamir-index` and `shamir-engine`):
```
cargo fmt -p <crate> -- --check
cargo clippy -p <crate> --all-targets -- -D warnings
./scripts/test.sh -p <crate>
```
If `fmt --check` fails, run `cargo fmt -p <crate>` (scoped, never `--all`).

## Scope discipline

- Do NOT design or implement an "operation ID + status" wire contract —
  report the sites you found needing this as context for the follow-up
  task instead.
- Do NOT touch the already-correct persist-then-recover state machines
  from #959/#961/#962/#966/#972 — only the error TEXT at the specific
  later-failure points.
- Do NOT add a rollback/undo mechanism for the durable persist step (e.g.
  attempting to un-register a Building index on backfill failure) — the
  review explicitly offers "honest rollback OR idempotent recovery" as
  alternatives, and idempotent recovery is the path already chosen and
  built this session; do not introduce a THIRD, uncoordinated mechanism.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

List every site found matching the pattern (file + function + which
persist-then-fail shape), the enriched error text you added at each, and
the test proving it. Explicitly list any DDL paths you checked that do NOT
have this gap (e.g. because a later step doesn't actually run after a
durable persist, or already has adequate context) so the orchestrator knows
the investigation was thorough, not partial. Give exact gate command output
per crate touched.

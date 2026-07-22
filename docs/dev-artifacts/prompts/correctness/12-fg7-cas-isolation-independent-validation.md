# FG-7: decouple `expected_version` CAS validation from Serializable/SSI

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## DECIDED CONTRACT (user, after an independent architectural consultation) — do not re-litigate

Option (d): give `expected_version` CAS its own independent validation
mechanism at commit time, so the "exactly one wins" guarantee holds
**regardless of isolation level** — Snapshot (implicit batches AND explicit
`.transactional()` without an explicit `.isolation(Serializable)`) and
Serializable alike. Do NOT auto-upgrade a batch's isolation to Serializable
(that was considered and rejected — it drags phantom-detection and
first-committer-wins write-set claims into ordinary non-CAS ops sharing the
same batch, and does not fix the explicit-Snapshot-with-CAS gap anyway).
This is engine core (commit path correctness) — the design below was
investigated and locked in before this brief was written; implement it
exactly, do not redesign the approach.

## Context — already investigated, do not re-derive

**The gap (found during FG-2 verification, `crates/shamir-server/tests/version_cas_e2e.rs`'s
own doc comment, and `docs/guide-docs/client-server-protocol-spec/OPTIMISTIC_CONCURRENCY.md`'s
"⚠️ Isolation caveat" section):** `check_expected_version`
(`crates/shamir-engine/src/table/write_exec.rs:62-91`) does a two-step
hybrid: (1) immediate `MvccStore::version_of` check, (2)
`tx.record_read_shared(...)` to register the key in the tx's SSI read-set
so the existing `validate_read_set` re-check at commit closes the race
window between step 1 and commit. But `record_read_shared`
(`crates/shamir-tx/src/tx_context.rs:490-500`) is a documented no-op unless
`self.isolation == IsolationLevel::Serializable`, and:
- `RepoInstance::run_implicit_batch_tx` (every plain non-transactional
  `client.execute(...)`) hardcodes `IsolationLevel::Snapshot`
  (`crates/shamir-engine/src/repo/repo_instance.rs`, `begin_tx` call site).
- The server's explicit-transaction default is ALSO `"snapshot"`
  (`crates/shamir-server/src/db_handler/tx_handlers.rs:26`) — a caller who
  writes `.transactional()` but forgets `.isolation(Serializable)` has the
  IDENTICAL silent hole.

Net effect: on both paths above, `expected_version` gets ONLY the immediate
stale-read check — no race-window backstop. Two concurrent writers that
both pass the immediate check before either commits can BOTH succeed,
silently violating "exactly one wins". Empirically reproduced already (see
the e2e test's doc comment).

**The existing Serializable machinery this decouples from**
(`crates/shamir-engine/src/tx/pre_commit.rs:436-450` Phase 2,
`crates/shamir-engine/src/tx/commit.rs:659-687` CRIT-4's `commit_lock`
guard around the validate→publish window) is gated on
`tx.isolation == IsolationLevel::Serializable` throughout. Do not touch
those Serializable-specific gates — this task ADDS a parallel,
isolation-independent validation path alongside them, it does not modify
the existing SSI contour.

## The design (implement exactly)

### 1. `TxContext`: a dedicated `cas_set` (`crates/shamir-tx/src/tx_context.rs`)

Add a new field, structurally mirroring `read_set`
(`scc::HashMap<(u64, Bytes), u64, THasher>`, line ~160):

```rust
/// FG-7: keys this tx staged an `expected_version` CAS check against,
/// independent of `read_set`/SSI. Validated at commit UNCONDITIONALLY
/// (any isolation level) — see `pre_commit_locked_validate`. Kept
/// separate from `read_set` so a CAS-specific commit-time failure maps
/// to `version_conflict`, distinct from a generic SSI `tx_conflict`.
pub cas_set: scc::HashMap<(u64, Bytes), u64, THasher>,
```

Initialize empty in `TxContext::new` (mirror `read_set`'s init, line
~282).

Add a recording method (mirrors `record_read_shared`'s shape, but
UNCONDITIONAL — no isolation gate):

```rust
/// FG-7: record a CAS check unconditionally (any isolation). Overwrite
/// semantics (last write wins) — unlike `read_set`'s first-read-wins,
/// there is no monotonic-conservative-bound concern here: `expected`
/// IS the exact version this specific check demands, not an observed
/// read version. (If the SAME key is CAS-checked twice in one tx with
/// two different `expected` values, that is almost certainly a caller
/// bug — the later check's `expected` is what commit validates.)
pub fn record_cas(&self, table_id: u64, key: Bytes, expected: u64) {
    self.cas_set.upsert_sync((table_id, key), expected);
}
```

Verify `scc::HashMap::upsert_sync` is the right method (insert-or-update);
if the API differs, use whatever `scc::HashMap` method achieves
insert-or-overwrite and document why.

### 2. `check_expected_version` (`crates/shamir-engine/src/table/write_exec.rs:62-91`)

Keep the existing immediate check AND the existing `record_read_shared`
call (under real Serializable it's still correct and cheap — no reason to
remove it). ADD a `tx.record_cas(table_token, id.to_bytes(), expected)`
call alongside them, unconditional (no isolation check — `record_cas`
itself has no gate).

### 3. `begin_tx` (`crates/shamir-engine/src/repo/repo_instance.rs`, ~line 776-803)

Currently sets `tx.set_version_provider(...)` ONLY when
`isolation == Serializable`. Change this to set it UNCONDITIONALLY (every
isolation level) — a `Snapshot` tx now also gets a `version_provider` so
`pre_commit_locked_validate`'s new CAS-validation phase (step 4 below) can
call `version_of` regardless of isolation. This is a thin `Arc` construction
+ clone, negligible cost, paid once per `begin_tx` regardless of whether
the tx ends up doing any CAS check.

### 4. `pre_commit_locked_validate` (`crates/shamir-engine/src/tx/pre_commit.rs:430-450`)

Add a NEW validation phase — call it "Phase CAS" in a comment — that runs
UNCONDITIONALLY (no `tx.isolation == Serializable` gate), placed logically
alongside Phase 2/2-bis:

```rust
// Phase CAS (FG-7): expected_version validation, independent of
// isolation level. Runs for EVERY tx (Snapshot and Serializable alike)
// whenever cas_set is non-empty — zero cost otherwise (empty-map check).
if !tx.cas_set.is_empty() {
    if let Some(provider) = tx.version_provider.as_ref() {
        // iterate cas_set, call provider.version_of(table_id, &key),
        // compare against the recorded `expected`; on ANY mismatch,
        // abort with the new TxError variant (see step 5) — do not
        // continue checking further keys once one fails (matches the
        // existing Phase 2 short-circuit-on-first-conflict style).
    }
    // If `tx.version_provider` is somehow `None` here (should not happen
    // after step 3's change, but defensive): decide and document whether
    // to skip validation (Debug: unreachable!/debug_assert) or fail safe
    // (treat as a conflict) — your call, document the choice.
}
```

Read the exact `scc::HashMap` iteration idiom already used for `read_set`
in `validate_read_set` (same file family) and mirror it — don't invent a
new iteration pattern.

### 5. `TxError` (`crates/shamir-engine/src/tx/commit.rs:107-129`)

Add a new variant distinct from `SsiConflict`:

```rust
/// FG-7: a CAS (`expected_version`) check failed at commit time — a
/// concurrent committer changed the key's version between the
/// immediate `check_expected_version` staging-time check and this tx's
/// commit. Surfaced to the wire path as `"version_conflict"` (NOT
/// `"tx_conflict"`) so client retry logic keyed on `version_conflict`
/// (the immediate-check error code) handles BOTH failure timings
/// identically — see `OPTIMISTIC_CONCURRENCY.md`.
#[error("cas conflict on key {key:?}: expected version {expected} but found {found}")]
CasConflict { key: bytes::Bytes, expected: u64, found: u64 },
```

### 6. Error-code mapping (TWO sites — both must be updated)

- `crates/shamir-engine/src/query/batch/batch_execute.rs:654-656` (or
  wherever `CommitError::SsiConflict`/`PhantomConflict`/`Wounded` map to
  `"tx_conflict"` — re-verify the exact current lines) — add
  `CommitError::CasConflict { .. } => "version_conflict".to_string()`.
- `crates/shamir-db/src/shamir_db/execute/db_tx.rs:253-257` (same mapping
  pattern, mirrored) — add the identical new arm.

Verify whether `CommitError` and `TxError` (step 5) are the same type
(re-exported under different names) or genuinely different types with a
manual `From`/mapping between them — if the latter, that mapping also
needs the new variant threaded through. Read both files' surrounding
context to confirm before assuming.

### 7. `commit.rs`'s CRIT-4 guard (`crates/shamir-engine/src/tx/commit.rs:659-687`)

Widen the condition that takes `gate.commit_lock()` for the validate→
publish window:

```rust
let _serializable_guard = if tx.isolation == IsolationLevel::Serializable
    || !tx.cas_set.is_empty()
{
    Some(gate.commit_lock().await)
} else {
    None
};
```

This gives a CAS tx (even under Snapshot) the SAME validate→publish
atomicity guarantee Serializable already relies on (CRIT-4's own comment
explains why this lock is load-bearing — read it in full). A Snapshot tx
with NO CAS keys is completely unaffected (empty-set check, no lock).

### 8. Docs

- `docs/guide-docs/client-server-protocol-spec/OPTIMISTIC_CONCURRENCY.md`:
  DELETE the "⚠️ Isolation caveat — step 2 requires `Serializable`" section
  entirely (not just edit it) — CAS is now correct on every isolation
  level and every entry path (implicit batch, explicit Snapshot, explicit
  Serializable). Add a short note instead: CAS's "exactly one wins" holds
  among writers that ALL use `expected_version` (the CAS protocol) — a
  non-CAS writer racing a CAS writer for the same key is still
  last-writer-wins, by design, same as any OCC system. This boundary is
  NOT changed by this task and should be stated plainly.
- `CHANGELOG.md`: one `[Unreleased]` bullet — this closes a real,
  previously-documented correctness gap (not a breaking change; opt-in
  `expected_version` behavior simply becomes fully correct on every path
  instead of only under an explicit `.isolation(Serializable)` opt-in).
- `docs/guide-docs/KNOWN_LIMITATIONS.md` §1 (Transactions): remove the
  `expected_version` CAS isolation caveat bullet added by FG-4 — the gap
  it describes is fixed by this task.

## Tests (MANDATORY)

1. **Flip** `crates/shamir-server/tests/version_cas_e2e.rs`'s
   `concurrent_cas_via_real_server_exactly_one_wins` test (or whichever
   test currently proves the gap on the PLAIN non-transactional path) —
   it should now ALSO assert "exactly one wins" for a plain
   non-transactional `client.execute(...)` batch (no `.transactional()` at
   all), not just under `.transactional().isolation(Serializable)`. Read
   the test's current doc comment in full first — it already documents
   the empirical reproduction; update it to describe the fix.
2. **New test**: an explicit `.transactional()` batch WITHOUT
   `.isolation(Serializable)` (i.e. explicit-Snapshot) with a racing CAS —
   must also show "exactly one wins" now (this is the wider gap `@fm`'s
   consultation surfaced — the FG-2 test suite likely never covered this
   exact combination).
3. **Regression**: confirm the EXISTING
   `concurrent_cas_exactly_one_wins` (engine-level, `version_cas_tests.rs`,
   under explicit `Serializable`) still passes unmodified — proves this
   task didn't disturb the pre-existing Serializable-path guarantee.
4. **Non-CAS Snapshot tx unaffected**: a plain non-transactional write
   with NO `expected_version` must NOT take the `commit_lock` (verify via
   whatever the cheapest observable proxy is — e.g. confirm two
   concurrent non-CAS writes to DIFFERENT keys still commit without
   waiting on each other; do not add invasive lock-instrumentation just
   for this test if a black-box timing/behavior assertion is sufficient).
5. **Error-code test**: the commit-time CAS failure path (a genuine race
   where step 1 passes for both writers but one loses at commit) surfaces
   `code == "version_conflict"` to the client, not `"tx_conflict"` — this
   is the specific error-mapping fix from step 6; write a test that
   reliably exercises the COMMIT-TIME failure (not just the immediate
   staging-time check) to prove the mapping, not just the common case.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @oracle --full` green (tx+engine).
- `./scripts/test.sh @server --full` green.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report: the exact `scc::HashMap` API used for `cas_set`'s upsert and
  iteration (confirm it matches `read_set`'s existing idiom or explain the
  difference), whether `CommitError`/`TxError` turned out to be the same
  type or needed a separate mapping, and literal gate output for all of
  the above.

If, after real investigation, any part of this design turns out
structurally harder than described (e.g. `scc::HashMap`'s iteration API
doesn't support what Phase 2's existing pattern assumes, or `CommitError`
vs `TxError` turns out to be a much bigger seam than expected) — do not
force a broken or silently-incomplete implementation. Report the precise
structural blocker so it can be triaged, rather than papering over it.

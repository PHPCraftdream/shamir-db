# Brief for #795 (F-5) — online storage migration API: gate as experimental, disabled by default

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Why this exists

The online storage-engine migration feature (`StartMigration` /
`CommitMigration` / `RollbackMigration` / `MigrationStatus` — wire-exposed
`BatchOp` variants, dispatched via
`crates/shamir-db/src/shamir_db/execute/admin_migration.rs`) has several
known, unfixed correctness gaps found in a release-readiness review:

- `MigrationShadowLog` is constructed at `StartMigration` time (see
  `handle_start_migration`, ~line 138: `Arc::new(MigrationShadowLog::new(...))`),
  but nothing in production code ever calls `shadow_log.append(ShadowOp...)`
  — the intent is presumably "writes landing on the source table between
  the initial snapshot copy and the final commit get intercepted and
  replayed onto the destination," but that interception is never wired up.
  Any write to the source table during an in-flight migration is silently
  lost from the destination. `handle_commit_migration` only compares
  RECORD COUNTS between source and destination, so an update that doesn't
  change the row count (e.g. an in-place field edit) isn't even detected
  as a discrepancy.
- Only `dst_engine: "in_memory"` is supported
  (`handle_start_migration` ~line 90-97) — not useful for a real
  durability-preserving migration.
- `MigrationCoordinator` state (`crate::engine::migration::MigrationCoordinator`)
  lives only in `ShamirDb::active_migrations` (an in-memory `DashMap`,
  `core.rs` ~line 71) — a server restart mid-migration loses all state,
  with no recovery path.
- The duplicate-start guard uses `try_lock`
  (`crates/shamir-engine/src/migration/coordinator.rs`, ~line 99) — lock
  CONTENTION (a legitimately concurrent, in-progress operation) is
  indistinguishable from "no migration running," so a `try_lock` failure
  is treated as `false` (not migrating), which can let a second
  `StartMigration` race in.
- Migration status is not retrievable after `CommitMigration` completes
  (nothing persists a terminal/history record), and there's no
  list-all-migrations capability despite the op family suggesting one.

None of this is fixed by this task — that would require write
interception, a durable state machine, and crash recovery (a much larger
effort, explicitly out of scope). Instead, this task makes the feature
**opt-in only, disabled by default**, so no client can trigger it without
an operator explicitly acknowledging the risk, and so the server (which
never opts in) cannot expose it to any regular client at all.

## The fix

1. Add an experimental-feature gate to `ShamirDb`
   (`crates/shamir-db/src/shamir_db/shamir_db/core.rs`): a new field, e.g.
   `pub(super) experimental_migration_enabled: Arc<std::sync::atomic::AtomicBool>`
   (default `false` at construction — check `ShamirDb::init`, ~line 130,
   and wherever else the struct's fields are initialized, likely a single
   constructor site given `Clone` derive + `Arc`-wrapped shared fields
   throughout this struct). Match the existing style in this file (this
   struct already has several `Arc<...>` shared-state fields with doc
   comments explaining their purpose — add one in the same style).
2. Add a public method to opt in, e.g.
   `pub fn enable_experimental_migration_api(&self)` that sets the flag to
   `true` (`Ordering::Relaxed` is fine — this is a coarse, rarely-toggled
   admin switch, not a hot-path atomic). Document on the method itself
   (doc comment) that this is UNSAFE for production use today — name the
   specific gaps above (write interception, in-memory-only backend,
   non-durable coordinator state) so a caller who reads the doc
   understands what they're opting into. This method has no "disable"
   counterpart needed for this task (once enabled for a process lifetime,
   it stays enabled — simplicity over a full toggle API).
3. In `handle_start_migration`
   (`crates/shamir-db/src/shamir_db/execute/admin_migration.rs`, ~line 19),
   check the flag FIRST (before the authorize call, or right after —
   whichever reads more naturally in the existing function; a disabled
   feature is arguably not even an authz question, so checking first and
   returning a clear `err_code("experimental_feature_disabled", ...)` — or
   whatever code convention this file already uses for similar rejections,
   check the existing `err`/`err_code` closures — before doing any other
   work is cleaner). The error message must reference
   `enable_experimental_migration_api` by name and briefly say why this is
   gated (not fully crash-safe / online-safe yet), so a caller hitting
   this in practice knows exactly what to do and why it matters.
4. **Do NOT gate `CommitMigration`/`RollbackMigration`/`MigrationStatus`**
   — once `StartMigration` is gated, no NEW migration can begin unless the
   flag is set, so these three ops become unreachable in practice for any
   caller who never called `enable_experimental_migration_api`. Gating
   only the entry point (`StartMigration`) is sufficient and keeps the
   diff minimal — do not add the same check redundantly to the other
   three handlers.
5. **The live server (`shamir-server`) must never call
   `enable_experimental_migration_api`** — verify (grep) that nothing in
   `crates/shamir-server/` currently calls it after you add it (it won't,
   since the method is new), and do NOT wire any server config flag to
   call it either — this task's scope is "the server never exposes this,
   full stop," not "make it server-configurable." If a future task wants
   a server-side opt-in config knob, that's separate work.

## Existing tests that call `StartMigration` successfully

Two files exercise the FULL `StartMigration` → `CommitMigration` success
path today:

- `crates/shamir-db/tests/migration_index2.rs` (3 tests: FTS/functional/
  vector index preservation across a migration) — these test genuinely
  useful behavior (do indexes survive a migration) that has nothing to do
  with the write-interception gap this task doesn't fix. Update this
  file's `setup()` helper (~line 32) to call
  `shamir.enable_experimental_migration_api()` right after constructing
  the `ShamirDb`, so these tests keep exercising the real mechanism
  end-to-end (this is exactly the "opt-in for internal testing, disabled
  for real clients" posture this task is going for).
- `crates/shamir-db/src/shamir_db/tests/execute_tests.rs` — check what
  this file's migration-related test(s) actually assert; if they test the
  success path, apply the same `enable_experimental_migration_api()` fix
  to their setup; if any test asserts specifically on
  `StartMigration`-without-opt-in behavior (unlikely, since this task is
  what's introducing that behavior), that's fine as-is.

## New tests to add

1. **`StartMigration` without the opt-in flag is rejected** — a fresh
   `ShamirDb` (no `enable_experimental_migration_api()` call) attempting
   `StartMigration` gets a clear, structured error (not a panic, not a
   generic error) naming the experimental gate.
2. **`StartMigration` after the opt-in flag succeeds** — calling
   `enable_experimental_migration_api()` then `StartMigration` proceeds
   normally (this is effectively covered by the updated
   `migration_index2.rs` tests already, but consider one small dedicated
   test at the `admin_migration.rs`/`execute_tests.rs` level too, for a
   test that specifically documents/pins the gate's on/off behavior
   rather than incidentally covering it via an index-preservation test).
3. Update `docs/guide-docs/KNOWN_LIMITATIONS.md` (or add a new entry if
   migration isn't covered there yet — check first) with a clear
   "experimental, opt-in only, disabled by default" note listing the
   unfixed gaps from the "Why this exists" section above, and mentioning
   `ShamirDb::enable_experimental_migration_api()` as the explicit opt-in.

## Constraints

- Do NOT attempt to fix the shadow-log write-interception gap, add a
  durable state machine, add crash recovery, add other `dst_engine`
  backends, fix the `try_lock` contention-vs-not-migrating ambiguity, or
  add a list-all-migrations capability — all explicitly out of scope,
  future work.
- Do NOT remove or weaken any of the THREE index-preservation tests in
  `migration_index2.rs` — they cover real, valuable behavior; just add the
  one-line opt-in call to their setup.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-db -p shamir-engine` and
  `cargo clippy -p shamir-db -p shamir-engine --all-targets -- -D
  warnings` must be clean for crates you touch.
- Follow workspace conventions: `use` at file top, `mod.rs` re-exports
  only, one primary export per file, surgical diff.

## Verification the orchestrator will run

```
cargo fmt -p shamir-db -p shamir-engine -- --check
cargo clippy -p shamir-db -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-engine --full
```

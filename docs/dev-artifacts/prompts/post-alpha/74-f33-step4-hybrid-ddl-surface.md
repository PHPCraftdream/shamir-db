# Brief for F-33 Step 4 (#838, P1) — wire the hybrid backend into the DDL surface

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-33 Step 2 (#836, `c32c4e3d`) added `BoxRepo::Hybrid` /
`BoxRepoFactory::Hybrid` (`crates/shamir-engine/src/repo/repo_types.rs`) —
an opt-in repo backend where `__info__`/`__interner__` mirror to a durable
fjall directory while `__data__`/`__history__`/`__tx__`/`__changelog__`
stay plain ephemeral in-memory. Step 3 (#837, `af4053da`) proved
`TableManager::create`'s open path already tolerates this correctly across
a restart. This step makes `hybrid` a real, user-reachable engine choice
via `CREATE REPO ... ENGINE 'hybrid'` — currently only `in_memory` and
`fjall` are wired.

**Read all three touch points below in FULL before editing** — they are
short, and each has an existing established pattern this step extends,
not redesigns.

## What to change

### 1. `crates/shamir-db/src/shamir_db/shamir_db/core.rs`

Three private helpers, all currently exhaustively matching over
`BoxRepoFactory`'s variants (adding `Hybrid` to that enum without updating
these breaks the build the moment the `fjall` feature is on, since `Hybrid`
is `#[cfg(feature = "fjall")]`-gated exactly like `Fjall`):

- **`factory_from_meta(engine: &str, path: Option<&str>) -> Option<BoxRepoFactory>`**
  (~line 694): add a `#[cfg(feature = "fjall")] "hybrid" => path.map(BoxRepoFactory::hybrid)`
  arm, same shape as the existing `"fjall"` arm right above it. This is the
  function that reconstructs a repo's factory from its PERSISTED system-store
  record on process boot (`core.rs:210-229` — read this call site: it reads
  `record["engine"]`/`record["path"]` and calls `factory_from_meta` to
  reattach every repo before recovery runs). A hybrid repo's persisted
  `path` is the fjall directory holding the `__info__`/`__interner__`
  mirror (there is no separate "data path" — data is ephemeral).
- **`extract_storage_type(factory: &BoxRepoFactory) -> String`** (~line 708):
  add `#[cfg(feature = "fjall")] BoxRepoFactory::Hybrid(_) => "hybrid"`.
  This is the counterpart that PERSISTS the engine string when a repo is
  first created (`db_management.rs:363-364` — read that call site too).
- **`extract_path(factory: &BoxRepoFactory) -> Option<String>`** (~line 722):
  add `#[cfg(feature = "fjall")] BoxRepoFactory::Hybrid(f) => Some(f.info_path.to_string_lossy().to_string())`.

### 2. `crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs`

`handle_create_repo`'s engine-selection match (~line 201-229) currently
handles `Some("in_memory")`, `Some("fjall") | None` (with a data_root
existence check, directory creation, and fallback to `in_memory()` when
there's no `data_root` — e.g. embedded/test `ShamirDb` instances have none),
and `Some(other) => error`.

Add a `#[cfg(feature = "fjall")] Some("hybrid") => { ... }` arm BEFORE the
catch-all `Some(other)` arm:

- Requires a `data_root` — unlike the `fjall`/`None` arm's silent fallback
  to `in_memory()` when there's no `data_root` (a sensible default for a
  DURABLE-BY-DEFAULT choice the caller didn't insist on), an EXPLICIT
  `ENGINE 'hybrid'` request with no `data_root` must be a clear, loud
  error — silently downgrading to fully-ephemeral would violate exactly
  the durability promise the caller explicitly asked for (this is a
  correctness/surprise concern, not a style preference: think through why
  before implementing, and say in your summary whether you agree this is
  the right call or found a reason it isn't).
- With a `data_root`: mirror the `fjall` arm's directory construction
  exactly — `root.join(&self.db_name)`, `create_dir_all`, then
  `.join(&op.create_repo)` — and call `BoxRepoFactory::hybrid(path)`
  instead of `BoxRepoFactory::fjall_raw(path)`.
- Also update the final `Some(other) => error` arm's message
  (`"Supported: in_memory, fjall."`) to list `hybrid` too.
- When the `fjall` cargo feature is OFF, `"hybrid"` should fall into the
  existing `Some(other)` unsupported-engine error path (same as `"fjall"`
  already does when that feature is off) — don't special-case it, this
  should fall out naturally from the `#[cfg(feature = "fjall")]` gate on
  your new arm.

### 3. Check for any OTHER exhaustive `BoxRepoFactory` match in `shamir-db`

Grep `crates/shamir-db/src` for other `match factory` / `match
.*BoxRepoFactory` sites beyond the 3 named above (reflection endpoints,
introspection/status commands, etc.) — if any exist and are exhaustive,
add the same `#[cfg(feature = "fjall")] BoxRepoFactory::Hybrid(...)` arm
there too, matching that site's existing style. State in your summary
whether you found any beyond the 3 already named.

## Tests

**MANDATORY, test-then-fix in the same commit.** Add to
`crates/shamir-db`'s existing DDL/repo test module (find the file(s) that
already test `CREATE REPO ... ENGINE '...'` for `fjall`/`in_memory` — follow
that convention, likely in `crates/shamir-db/tests/` or a `src/.../tests/`
directory; use the query builder, never hand-built JSON, per this repo's
"query construction — builder only" rule):

1. `CREATE REPO ... ENGINE 'hybrid'` against a `ShamirDb` WITH a
   `data_root` succeeds; a subsequent write to a table's index/validator
   in that repo, then a full `ShamirDb` restart against the SAME
   `data_root` (simulating a process restart, the same way this crate's
   existing fjall-persistence tests already do — check
   `declarative_schema_fk_autocommit_e2e.rs` or similar for the restart
   pattern this crate uses), still shows the index/validator config, but
   an inserted row is gone (data is ephemeral) — this is the DDL-surface-level
   equivalent of Step 3's lower-level proof, now through the REAL
   `CREATE REPO`/`ShamirDb::new`/reattach-on-boot path rather than a
   hand-built `BoxRepoFactory::hybrid(...)` call.
2. `CREATE REPO ... ENGINE 'hybrid'` against a `ShamirDb` with NO
   `data_root` (an in-memory-only `ShamirDb`, however this crate's test
   helpers construct one) returns the clear error from point 1 of section
   2 above, not a silent `in_memory()` downgrade.
3. `extract_storage_type`/`extract_path` round-trip: create a hybrid repo,
   confirm the persisted system-store record's `engine` field reads
   `"hybrid"` and `path` is the expected directory (check however the
   existing fjall-repo persistence test confirms this, and mirror it).
4. Unsupported-engine error message mentions `hybrid` alongside
   `in_memory`/`fjall` (a simple string-contains assertion is enough).

## Constraints

- Do NOT touch `BoxRepo`/`BoxRepoFactory`/`HybridRepoComposite` in
  `shamir-engine` (Step 2, already landed) — this step is purely the
  `shamir-db`-side DDL wiring.
- Do NOT add a hybrid entry to any client-facing SDK/builder enum of
  engine names UNLESS one already exists for `fjall`/`in_memory` as a
  typed (non-string) choice — check the Rust/TS query builders for an
  `engine` enum; if it's a plain string today (as `CreateRepoOp.engine:
  Option<String>` in `shamir-query-types` suggests), leave it a string —
  don't introduce a new typed enum as part of this task.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-db -- --check` and
  `cargo clippy -p shamir-db --all-targets -- -D warnings` must be clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-db -- --check
cargo clippy -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -- hybrid
./scripts/test.sh -p shamir-db --full
```

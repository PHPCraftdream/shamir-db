# Documentation Accuracy 6c — fix stale redb→fjall references (03-storage.md + doc comments)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Third item of "Этап 6 — Documentation accuracy"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 09. `redb` was the storage backend used before the
project migrated to `fjall`. **Confirmed via investigation**: `redb` is
now completely gone from the dependency tree (zero `redb` package in
`Cargo.lock`), and `shamir-storage`'s own `[features]` section
(`crates/shamir-storage/Cargo.toml`) has no `redb` feature at all —
only `fjall`/`all-backends`. The actual runtime repo-creation dispatchers
(`crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:218`,
`crates/shamir-db/src/shamir_db/shamir_db/core.rs:646`,
`crates/shamir-db/src/shamir_db/system_store.rs:94`) all call
`BoxRepoFactory::fjall`/`fjall_raw` — there is no `BoxRepoFactory::redb`
method anywhere in the codebase.

**⚠ Important scoping note — this is NOT a repo-wide "replace every redb
mention" sweep.** A `grep -rli redb` across the repo turns up **~150+
files**, but the overwhelming majority are:
- Historical checkpoint/audit/perf-journal docs under
  `docs/dev-artifacts/checkpoints/`, `docs/dev-artifacts/perf/`,
  `docs/dev-artifacts/ops/` — these are dated historical records of what
  was true AT THE TIME (e.g. a 2026-06-19 perf snapshot comparing redb vs
  fjall benchmarks) and must NOT be rewritten to claim something different
  happened than what the record shows.
- Legitimate storage-engine-design **comparison lists** in real Rust doc
  comments, e.g. `crates/shamir-storage/src/types.rs` lines ~138/182/262/293
  say things like "backends (redb, sled, fjall, persy, nebari, canopy)
  override this trait method the following way" — these are general
  architectural rationale comments about the DESIGN SPACE of storage
  engines in the abstract, not claims that THIS project currently uses
  redb. Leave these untouched.
- Literal test-fixture file NAMES like `meta.redb`/`db.redb` used as path
  strings in `crates/shamir-db/tests/*.rs` and
  `crates/shamir-storage/test_data/*.redb` — these are just filenames
  (the `.redb` extension is cosmetic legacy naming for a test artifact,
  not a claim about the backend). Do NOT touch test code or rename test
  fixture files — out of scope for a docs-only brief and risks breaking
  tests for zero reader-facing benefit.
- `docs/dev-artifacts/roadmap/DURABLE_BY_DEFAULT.md` and other
  already-shipped design-decision docs describing what was PLANNED/
  IMPLEMENTED at the time of writing (e.g. its §D2 literally specs
  `BoxRepoFactory::redb(path)` as the intended implementation) — these
  are historical design records, not living reference docs; leave them
  alone (mirroring how this campaign's earlier doc-accuracy tasks, e.g.
  6a's `LIVE_SUBSCRIPTIONS.md` fix, updated a STATUS HEADER rather than
  rewriting historical design prose).

**What's actually in scope — genuine, currently-misleading examples that
a reader would try to use and it would fail:**

1. `docs/guide-docs/guide/03-storage.md` line 70: "Durable по умолчанию
   ... wire-созданные репозитории — durable (redb)." — should say fjall.
   Also line 81, a stray inline HTML comment:
   `<!-- TODO: verify that wire-created durable repo engine=redb path is
   auto-derived from data_root per DURABLE_BY_DEFAULT.md D2 -- currently
   CreateRepoOp.engine is Option<String> with None default -->` — verify
   against `crates/shamir-query-types/src/admin/types/repo_ops.rs`
   (`CreateRepoOp.engine: Option<String>`) and the actual dispatcher
   (`admin_db_repo.rs`) whether this TODO is now resolved/answered by
   current code; if so, either remove the stale TODO or update it to
   reference `fjall` and state the answer plainly instead of leaving an
   open question that's actually been resolved.
2. `crates/shamir-storage/src/lib.rs` line 9 — a crate-level doc-comment
   USAGE EXAMPLE: `//! shamir-storage = { version = "0.1",
   default-features = false, features = ["redb"] }`. This is a real bug:
   copy-pasting this into a `Cargo.toml` today would fail (`redb` isn't a
   real feature of this crate anymore) — fix to `features = ["fjall"]`.
3. `crates/shamir-db/Cargo.toml` line 12 — comment:
   `# minimal builds: default-features = false, features = ["redb"].` —
   same bug, same fix (→ `["fjall"]`).
4. `crates/shamir-engine/Cargo.toml` line 61 — comment:
   `#   shamir-engine = { default-features = false, features = ["redb"] }`
   — same bug, same fix.
5. `crates/shamir-server/src/config.rs` line 74 — doc comment on
   `Config.data_dir`: "Root directory for durable state (server_meta,
   user_directory, audit log, redb databases)." → should say `fjall`.
6. `crates/shamir-engine/src/repo/README.md` line ~77 — an ACTUAL API
   USAGE EXAMPLE showing BOTH `BoxRepoFactory::redb("./data/main.redb")`
   AND `BoxRepoFactory::fjall("./data/fjall")` side by side (as if both
   are valid choices today). Confirmed via grep: `BoxRepoFactory::redb`
   does not exist as a method anywhere in
   `crates/shamir-engine/src/repo/repo_types.rs` (only `fjall`/
   `fjall_raw` constructors exist). This example would fail to compile.
   Remove the `redb` line from this example entirely (don't just rename
   it to fjall if fjall is already shown on the next line — check the
   surrounding prose to decide the cleanest fix, since there may be two
   near-duplicate examples now).

## The task

1. Fix each of the 6 sites above precisely, replacing the stale `redb`
   claim/example with the accurate `fjall` equivalent (or removing/
   resolving a stray TODO, per site 1's second half).
2. Before editing each Cargo.toml/lib.rs comment, confirm your own
   understanding by re-running `grep -n "redb" crates/shamir-storage/
   Cargo.toml` (or the equivalent for whichever file) yourself and
   checking the actual current `[features]` list — don't take this
   brief's citations on faith, verify against the file as it exists now.
3. Do a final scoped re-grep — `grep -rn "redb" docs/guide-docs/
   crates/*/Cargo.toml crates/*/src/lib.rs crates/*/README.md
   crates/*/src/*/README.md` (adjust paths to cover doc-comment/README
   surfaces, NOT test files or historical dev-artifacts docs) — and
   confirm no OTHER genuinely-misleading "redb" reference in a
   user-facing doc/README/doc-comment surface was missed. If you find
   one not listed above, fix it too and report it as an addition beyond
   the brief's original list.

## Out of scope

- Do NOT touch `docs/dev-artifacts/checkpoints/`, `docs/dev-artifacts/
  perf/`, `docs/dev-artifacts/ops/`, or `docs/dev-artifacts/roadmap/
  DURABLE_BY_DEFAULT.md` (or any other historical/dated design-decision
  doc) — these are historical records, not living reference docs.
- Do NOT touch `crates/shamir-storage/src/types.rs`'s general
  storage-engine-comparison rationale comments (redb/sled/fjall/persy/
  nebari/canopy listed together as examples of the design space) — these
  are not claims about this project's current backend.
- Do NOT touch any `.rs` test file's literal `meta.redb`/`db.redb` path
  string, or the `.redb` test-fixture binary files under `test_data/` —
  renaming these is a code change with real (if small) risk, not a docs
  fix, and the filename itself doesn't mislead a reader of documentation.
- Do NOT touch anything from the already-completed Этапы 1-5 or tasks
  6a/6b — this brief is scoped to the 6 sites listed above (plus
  whatever your final re-grep pass in step 3 turns up on user-facing
  doc/README/doc-comment surfaces specifically).

## Verification (MANDATORY before you report done)

- No `cargo test`/`clippy` gate applies for the `.md`/`README.md` sites,
  but for the THREE Rust-file sites (`shamir-storage/src/lib.rs` doc
  comment, `shamir-db/Cargo.toml` comment, `shamir-engine/Cargo.toml`
  comment, `shamir-server/src/config.rs` doc comment) run
  `cargo fmt -p shamir-storage -p shamir-db -p shamir-engine
  -p shamir-server -- --check` afterward to confirm the comment edits
  didn't introduce a formatting drift (doc comments can trip fmt if
  line-wrapped wrong).
- Report the final re-grep from step 3's output and confirm every
  remaining `redb` hit in the scoped surfaces (docs/guide-docs,
  Cargo.toml comments, lib.rs/README.md doc surfaces) is now either
  fixed or confirmed out-of-scope (historical doc / general comparison
  list) per the categories above.
- Confirm explicitly what you decided for `03-storage.md`'s stray TODO
  at line 81 — removed as resolved, or updated with corrected wording —
  and why.

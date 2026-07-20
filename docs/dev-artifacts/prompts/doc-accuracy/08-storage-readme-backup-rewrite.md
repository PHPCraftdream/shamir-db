# Rewrite `shamir-storage/src/README.md` + fix `shamir-server/src/backup.rs`'s stale redb content

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Standalone cleanup item, discovered during task 6c (redb→fjall doc-accuracy
sweep, commit `6645cb06`) but deliberately left out of that brief's narrow
scope since it needs a holistic rewrite, not a targeted one-line swap.

**This is NOT a simple find-replace of `redb`→`fjall`.** The investigation
for this brief found the drift is much deeper: `shamir-storage/src/
README.md` describes **SIX** on-disk backends (Sled, Redb, Fjall, Nebari,
Persy, Canopy) — **five of those six do not exist in the codebase at all**,
not even behind a disabled feature flag. Verify this yourself first:

```
ls crates/shamir-storage/src/            # actual files
grep -n "^pub mod\|^mod\|cfg(feature" crates/shamir-storage/src/lib.rs
grep -n "^\[features\]" -A 20 crates/shamir-storage/Cargo.toml
```

You will find: `error.rs`, `key_bytes.rs`, `storage_cached.rs`,
`storage_in_memory.rs`, `storage_membuffer.rs`, `types.rs` (always
compiled), and `storage_fjall.rs` (the ONLY concrete on-disk backend,
gated `#[cfg(feature = "fjall")]`, enabled by the `all-backends` meta-
feature which today expands to `["fjall"]` alone — read the Cargo.toml
comment explaining this). There is no `storage_sled.rs`/`storage_redb.rs`/
`storage_nebari.rs`/`storage_persy.rs`/`storage_canopy.rs` anywhere in the
tree, and no `sled`/`redb`/`nebari`/`persy`/`canopy` cargo feature. The
README's file tree, backend comparison table, "Backend-Specific Details"
section, cursor-management table, and `cargo test test_sled`/`test_redb`/
etc. block are **all describing a codebase state that no longer exists.**

**`storage_membuffer.rs` is not mentioned in the README at all** — read its
module doc (`crates/shamir-storage/src/storage_membuffer.rs`, top of file)
to understand its real role (a moka-based concurrent write-back cache
wrapper — read the doc comment's own "Why moka" section for the design
rationale) and how it relates to `storage_cached.rs` (also a wrapper —
investigate whether both are still live/used-by-something-real, or whether
one has superseded the other; check callers of each via grep before writing
anything, don't assume).

## The task — Part 1: `shamir-storage/src/README.md`

**Full rewrite**, not a patch. Read the CURRENT file first
(`crates/shamir-storage/src/README.md`) to see the existing structure/tone
worth preserving, then rebuild each section against the REAL codebase:

1. **File tree**: list only files that actually exist (`error.rs`,
   `key_bytes.rs`, `storage_in_memory.rs`, `storage_cached.rs`,
   `storage_membuffer.rs`, `types.rs`, `storage_fjall.rs`), with correct
   "always compiled" vs "feature-gated" annotations (check every file
   against `lib.rs`'s actual `#[cfg(feature = ...)]` attributes — don't
   assume from the current README's annotations, they may ALSO be stale
   even for files that DO exist).
2. **`Store`/`Repo` trait sections**: read `crates/shamir-storage/src/
   types.rs` in full and reproduce the ACTUAL current method signatures —
   the current README shows only 4 `Store` methods
   (`insert`/`set`/`get`/`remove`) plus the two stream methods; the real
   trait additionally has `get_many`, `set_no_flag`, `remove_no_flag`,
   `insert_many`, `set_many`, `remove_many` (check which are provided
   default-impl methods vs required — note them accordingly). Do the same
   check for the `Repo` trait.
3. **Backend comparison table / "Supported Backends"**: replace with the
   real, current set — `InMemory` (always compiled), `Cached` (wrapper,
   always compiled), `MemBuffer` (wrapper, always compiled — describe its
   real role per your investigation above), `Fjall` (the one on-disk
   backend, feature-gated). Do NOT invent "Status" claims (e.g. "✅
   Stable") — either verify each claim against real test coverage
   (`crates/shamir-storage/src/tests/`) or drop the column if you can't
   substantiate it.
4. **"Backend-Specific Details" section**: keep ONLY `InMemoryStore`,
   `CachedStore`, `MemBufferStore` (new), and `Fjall` — delete the Sled/
   Redb/Nebari/Persy/Canopy subsections entirely (not "rename to Fjall",
   they describe engines that were never fjall to begin with — Sled's
   `skip_first` cursor quirk, Redb's `Bound` API, Nebari's `ScanEvaluation`,
   Persy's `PersyId` mapping, Canopy's LZ4 compression are all genuinely
   about DIFFERENT, absent engines, not stale names for fjall).
5. **"Async Streaming Implementation" / "Cursor Management" section**:
   verify the code example still matches `storage_fjall.rs`'s actual
   `iter_stream` implementation (read it) — fix the example if it's drifted,
   and replace the per-backend cursor-management table with just Fjall's
   real behavior (read `storage_fjall.rs`'s streaming code to describe it
   accurately, don't guess).
6. **"Testing" section's `cargo test test_sled`/`test_redb`/etc. block**:
   this repeats the SAME raw-`cargo test` anti-pattern task 6b already
   fixed elsewhere in the repo (README.md/CONTRIBUTING.md/CLAUDE.md) —
   replace with the correct `./scripts/test.sh`/`cargo tl` invocation per
   this repo's CLAUDE.md "Centralised test entry point" section, scoped to
   this crate (e.g. `./scripts/test.sh -p shamir-storage`), and drop the
   per-backend test-name list (there is only one backend to test now).
7. **"Performance Considerations" / "Choosing a Backend" table**: same
   fix — only real options remain (InMemory for testing, Fjall for
   persistence, Cached/MemBuffer as caching wrappers). Don't invent
   comparative claims between backends that no longer exist.
8. **"Future Enhancements" checklist** at the bottom: verify each item
   against the actual codebase before keeping it (e.g. "Transactions
   (Persy-only)" is now meaningless — check whether the workspace ALREADY
   has transactions via `shamir-tx` at a higher layer, in which case this
   whole item is stale/misleading about where transactions actually live).

## The task — Part 2: `shamir-server/src/backup.rs`

Four stale references (module doc comment, lines ~5, ~9-14, ~17, and the
`copy_dir_recursive` function's line ~135 inline comment) all say "redb"
where the actual backend is fjall. **Do not just swap the word** — the
technical claims are engine-specific and may not transfer:

- Line ~9-14's claim ("redb 3.x doesn't expose that API on `Database`
  directly"; "redb's per-page CRC32 + atomic-commit design... recoverable
  as the pre-commit state on the next open") is describing REDB's specific
  page-based storage engine properties. Fjall is an LSM-tree (journal-based
  writes, `PersistMode` — check `storage_fjall.rs`'s imports/usage of
  `fjall::PersistMode` for the real durability model). **Investigate what
  fjall ACTUALLY guarantees for a stop-and-copy backup** (does it expose an
  online/live backup API? what does a mid-flush copy of its on-disk files
  recover to on next open — check fjall's own docs/README if vendored, or
  its crate docs, or `storage_fjall.rs`'s own comments about
  `PersistMode`/durability) and write the ACCURATE equivalent reasoning for
  why stop-and-copy is still the right choice (or isn't — report if you
  find fjall actually makes this SAFER or DIFFERENT in some material way,
  don't just launder the same conclusion with a new noun).
- Line ~17's "Future enhancement... via `system_store`'s existing redb
  handle" — grep for `system_store` across `shamir-server`/`shamir-db` to
  confirm whether this identifier refers to something real (it may be a
  forward-looking/aspirational name, not an actual current symbol — check
  before deciding whether to keep, rename, or generalize this sentence).
- Line ~135's "A redb file shouldn't normally be one of these [symlinks/
  sockets/fifos]" — straightforward rename to fjall's actual on-disk file
  shape (check what files `storage_fjall.rs`/fjall's keyspace actually
  creates on disk, so the comment names the right thing, not just s/redb/fjall/).

## Verification (MANDATORY before you report done)

- Every factual claim in the rewritten `README.md` must be traceable to a
  real file/symbol/test you actually checked — cite what you verified in
  your summary (file:line for each major claim), the same discipline this
  campaign's earlier doc-accuracy tasks (6a/6c/6f) used.
- `cargo build -p shamir-storage` and `cargo build -p shamir-server` clean
  (docs-only changes shouldn't break the build, but confirm nothing you
  touched was actually live code — these are `.rs` doc-comment edits in
  `backup.rs`, so a build check is cheap insurance).
- `./scripts/test.sh -p shamir-storage -p shamir-server --full` green (no
  behavior change expected — doc comments only in `backup.rs`; the README
  is not compiled).
- `cargo fmt -p shamir-server -- --check` clean (in case `backup.rs`'s
  comment edits touch formatting-sensitive lines).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.

## Out of scope

- Do NOT touch any other crate's docs or any Этап 1-8 / task #695/#729
  artifacts (all already landed, unrelated).
- Do NOT add a `sled`/`redb`/`nebari`/`persy`/`canopy` feature flag or any
  code to "restore" those backends — this is a documentation-accuracy fix
  only, reflecting the codebase as it actually is today.
- If you find `storage_cached.rs` is genuinely dead/superseded code (not
  just under-documented), do NOT delete it in this task — flag it in your
  summary as a possible follow-up, but this brief is scoped to
  documentation, not code removal.

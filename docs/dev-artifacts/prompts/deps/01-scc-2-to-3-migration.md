בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# scc 2.2 → 3.8.4 migration (RUSTSEC-2026-0205)

## Context

`cargo deny check` finally ran to completion for the first time in this
repo's history (a prior bug — an absolute local path in
`[workspace.dependencies].bench-scale-tool` — broke `cargo metadata`
before the advisory scan could even start; already fixed separately).
It surfaced a real, previously-invisible finding:

`RUSTSEC-2026-0205` — `scc`'s `Array::insert` is not exception-safe: if a
user-provided `K::compare` panics mid-insert, `mem::forget` has already
run and the destructors that fire during unwind cause a double-free.
Fixed upstream in `scc 3.8.4` (uses `ManuallyDrop` instead of
`mem::forget`). No fix exists in the 2.x line.

**Risk assessment (already done, do not re-litigate):** every `K` type
`scc::HashMap`/`scc::TreeIndex` is keyed by in this workspace is a plain
type (`String`, `u64`, tuples of these, `RecordId`) with a derived/
standard `Eq`/`Ord` — none has a custom `compare` that can reasonably
panic. Real exploitability here is low. The migration is being done
anyway because scc is this database's core concurrency primitive and we
want the upstream fix, not because of active concern.

## What changed between scc 2.2 and 3.8.4

Confirmed by building scc 3.8.4's own docs locally
(`cargo doc -p scc@3.8.4 --no-deps`) and comparing to the 2.2 API errors
surfaced by `cargo check`:

1. **Every method that had both a blocking and an async form now needs an
   explicit suffix.** In 2.x the bare name (`insert`, `remove`, `get`,
   `read`, `entry`, `contains`, `clear`, `retain`, `any`, `update`,
   `upsert`, `iter`, `iter_mut`) was the SYNC/blocking form, and the async
   form was already suffixed `_async` (e.g. `insert_async`). In 3.x the
   sync form now ALSO needs its own explicit suffix: `insert` → `insert_sync`,
   `get` → `get_sync`, `remove` → `remove_sync`, `read` → `read_sync`,
   `entry` → `entry_sync`, etc. **The already-`_async`-suffixed call sites
   in this codebase do NOT need to change** — `insert_async`,
   `remove_async`, `entry_async`, `retain_async`, `remove_if_async` etc.
   still exist with the same names/semantics in 3.8.4.
2. **This suffix rule does NOT apply uniformly to every scc type.**
   `scc::TreeIndex` in 3.8.4 kept `clear`, `contains`, `iter`, `len`,
   `is_empty`, `peek*`, `range`, `locate`, `depth` bare (no sync/async
   split — these were always synchronous, lock-free reads/iteration).
   Only `TreeIndex`'s write-ish methods that had both forms
   (`insert`/`remove`/`remove_if`/`remove_range`/`read`/`upsert`) got the
   `_sync`/`_async` split. `scc::HashMap` got the full treatment (every
   listed method suffixed).
3. **`scc::ebr::Guard` moved to the crate root: `scc::Guard`.** The `ebr`
   module no longer exists (`error[E0433]: could not find 'ebr' in
   'scc'`). Every `scc::ebr::Guard::new()` becomes `scc::Guard::new()`;
   every `use scc::ebr::Guard;` becomes `use scc::Guard;` (or
   `use scc::ebr::Guard as Guard;` style imports likewise drop the `ebr::`
   segment).
4. `scc::hash_map::Entry` (the `Occupied`/`Vacant` enum used in `match`
   patterns like `scc::hash_map::Entry::Occupied(e)`) is UNCHANGED — do
   not touch those match arms, only the method call that PRODUCES the
   entry (`.entry(...)` → `.entry_sync(...)`, `.entry_async(...)` stays).

## The Cargo.toml side is ALREADY DONE

These 7 files already have `scc = "3.8"` (bumped from `"2.2"`) — do not
re-touch them, do not touch any other dependency in them:

```
crates/shamir-client/Cargo.toml
crates/shamir-engine/Cargo.toml
crates/shamir-funclib/Cargo.toml
crates/shamir-server/Cargo.toml
crates/shamir-storage/Cargo.toml
crates/shamir-tx/Cargo.toml
crates/shamir-wasm-host/Cargo.toml
```

(`captrack`, an external published dependency pinned at `=0.1.0`, still
pulls its own internal `scc 2.4.0` — that's fine, a duplicate transitive
version is not your concern, do not try to "fix" it.)

## The task: fix every call site the compiler flags

**Compiler-guided, not blind grep-replace.** Run:

```
cargo check --workspace --all-targets
```

Every error will be one of:

- `error[E0599]: no method named 'X' found for struct 'scc::HashMap<K, V, H>'`
  or `... for reference '&scc::TreeIndex<...>'` or `... for struct
  'std::sync::Arc<TreeIndex<...>>'` — the fix is renaming that call site's
  method from `X` to `X_sync` (verify `X_sync` actually exists on that
  type per the exceptions in point 2 above — it will, for every error
  `cargo check` surfaces from THIS specific 2.2→3.8 change).
- `error[E0433]: could not find 'ebr' in 'scc'` — the fix is deleting the
  `ebr::` path segment (`scc::ebr::Guard` → `scc::Guard`,
  `use scc::ebr::Guard` → `use scc::Guard`).
- `error[E0282]: type annotations needed` — this one already showed up in
  `shamir-storage` before any other fix; it is very likely a knock-on
  effect of an `insert`/`get` rename changing what the compiler can infer
  — fix the nearby rename first, then re-check if this error persists on
  its own merit (if it does, add the minimal explicit type annotation
  the compiler asks for; do not restructure the surrounding code).

**Iterate**: fix the errors `cargo check` reports, re-run, repeat, until
`cargo check --workspace --all-targets` is clean. Then run
`cargo clippy --workspace --all-targets -- -D warnings` and fix anything
it flags in files you touched (do not fix pre-existing unrelated clippy
lints in files you didn't otherwise touch for this task).

## Known-affected files (from the pre-migration `scc::`/`use scc` grep —
use this as a checklist, not a boundary; trust the compiler over this list
for completeness)

```
crates/shamir-client/src/interner_cache.rs
crates/shamir-engine/src/repo/repo_instance.rs
crates/shamir-engine/src/repo/version_provider.rs
crates/shamir-engine/src/tx/drainer.rs                  (heavy scc::ebr::Guard use)
crates/shamir-engine/src/validator/registry.rs
crates/shamir-funclib/src/scalar_resolver.rs
crates/shamir-index/src/registry.rs
crates/shamir-index/src/vector/hnsw_adapter.rs          (heaviest single file)
crates/shamir-index/src/vector/snapshot.rs
crates/shamir-server/src/replication/supervisor.rs
crates/shamir-server/src/subscriptions/bridge.rs
crates/shamir-server/src/subscriptions/decode_cache.rs
crates/shamir-server/src/subscriptions/deliver_cache.rs
crates/shamir-server/src/subscriptions/registry.rs
crates/shamir-server/src/user_directory.rs
crates/shamir-storage/src/storage_cached.rs
crates/shamir-storage/src/storage_in_memory.rs
crates/shamir-tx/src/completion_tracker.rs
crates/shamir-tx/src/layered_interner.rs
crates/shamir-tx/src/mvcc_store/mod.rs
crates/shamir-tx/src/mvcc_store/mvcc_gc.rs
crates/shamir-tx/src/mvcc_store/mvcc_history.rs
crates/shamir-tx/src/mvcc_store/mvcc_locks.rs
crates/shamir-tx/src/repo_tx_gate.rs
crates/shamir-tx/src/tx_context.rs
crates/shamir-tx/src/versioned_overlay.rs
crates/shamir-wasm-host/src/context.rs
crates/shamir-wasm-host/src/registry.rs
```

Plus any `tests/`/`benches/` files under these crates that directly
touch `scc::`/`scc::ebr` (the compiler will find these too once you run
`--all-targets`, which includes tests and benches).

## Verification (MANDATORY before reporting done)

1. `cargo fmt --all -- --check` clean (or `cargo fmt -p <touched crate>`
   if drift appears — do NOT run a workspace-wide `cargo fmt --all`
   rewrite, only format the files you touched).
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `./scripts/test.sh` (full lib suite, ALL crates — this touches nearly
   every crate in the workspace, do not scope narrowly). **Never** use
   raw `cargo test` — it is blocked by this repo's cargo-runner guard;
   only `./scripts/test.sh` (wraps `cargo nextest`) is sanctioned.
4. Then `./scripts/test.sh --full -p shamir-tx -p shamir-engine -p
   shamir-server -p shamir-storage -p shamir-index -p shamir-wasm-host`
   (integration tests for every crate this migration touches — the
   concurrency-sensitive suites live here: MVCC store tests, drainer
   tests, tx_gate tests, versioned_overlay tests, interner concurrency
   tests).
5. Watch specifically for **hangs/timeouts**, not just failures — a
   subtly wrong `_sync` vs `_async` choice (e.g. accidentally calling the
   async form's blocking equivalent from inside an async context, or
   vice versa) could deadlock rather than fail loudly. If nextest reports
   `TIMEOUT` on anything under `shamir-tx`/`shamir-engine`/`shamir-server`,
   that is a genuine regression from this migration — do not raise
   timeouts to paper over it, find the actual call site.
6. Run the affected concurrency benches at least once each (not for
   numbers, just to prove they still execute end-to-end without hanging):
   `cargo bench -p shamir-engine --bench tx_concurrent`,
   `cargo bench -p shamir-engine --bench interner_concurrent`,
   `cargo bench -p shamir-engine --bench membuffer_concurrent`. Quick
   mode is fine (this repo's bench harness defaults to a fast calibrated
   run — see `CLAUDE.md` bench section, no special flags needed for a
   sanity pass).

## Scope discipline

- Touch ONLY the scc-related call sites this migration requires. Do not
  refactor, rename, reformat, or "improve" surrounding code.
- Do not touch `Cargo.toml` files — already done (see above).
- Do not touch `deny.toml`, `.github/workflows/*`, or any other
  supply-chain gate config — out of scope for this brief.
- Do not add new files. Every change is an edit to an existing file.

## ⛔ Git discipline (MANDATORY, verbatim)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

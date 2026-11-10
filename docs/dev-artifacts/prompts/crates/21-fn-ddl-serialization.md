# Brief: serialize function DDL per name (SEC-FN-DDL)

## Why

The catalogue write paths of stored functions are read-modify-write sequences with no mutual exclusion and no
compare-and-swap. Found by the security review of `crates/19-fn-catalogue-security.md` (landed):

* `facade/alter.rs` (~38-87, `alter_function_optimize` / rebuild): loads the existing row, then saves the row and
  replaces the registry entry with the OLD artifact id. A concurrent management replace by the owner (new code, or a
  downgrade from Definer to Invoker) can be overwritten by this stale write: either a rollback to the old Definer
  version, or a permanent fail-closed mismatch between the row and the registry entry (and the old version again after
  a reboot).
* `management.rs` rename (~366-401): `old_record` is read before the move and saved at the destination afterwards.
* `access/meta.rs` (~353-363): `set_resource_meta` (chmod/chown/chgrp) for a function row is another stale
  read-modify-write.
* `facade/mark_corrupt.rs` (~21-29): replaces the registry entry with the corrupt marker keeping an id, so a healthy
  artifact installed in between is reported corrupt.
* Two concurrent management replaces, or a create race where the row save succeeds and the registry registration
  fails, can also leave a permanent row/entry mismatch.

Invocation paths (root and nested) already fail closed on a mismatch; this task removes the way to produce one and the
lost-update/rollback hole. Read the previous brief and `db/function/{management.rs,artifact.rs,invocation.rs}`,
`facade/*`, `boot/load.rs`, `access/meta.rs` first.

## What (decisions are made; do not re-open them)

1. **Per-name async DDL serialization.** Add a small component owned by `ShamirDb` that hands out one
   `tokio::sync::Mutex<()>` per function name (a `scc::HashMap<String, Arc<tokio::sync::Mutex<()>>, THasher>`; this
   is the sanctioned "guard across `.await`, bounded contention" primitive of the code ideology, DDL-only, never on a
   read/invoke path). Entries are removed when no longer referenced (no unbounded growth: drop the map entry when the
   guard is released and the `Arc` is the last reference; say how you made that race-free). Acquire it for the whole
   read-modify-write sequence of: create/replace (`create_function*`), drop, rename (take BOTH names, in a fixed total
   order of the names, to avoid deadlock; same name twice is a no-op), alter/rebuild/optimize, `mark_function_corrupt`,
   and `set_resource_meta` when the path is a function (chmod/chown/chgrp). Authorization checks stay before taking the
   lock where they are today, but every row read that feeds a write happens AFTER acquiring the lock (re-load inside
   the lock; never reuse a row read before it).
2. **Registry write conditional on the entry.** `mark_function_corrupt` and alter's registry replacement must replace
   the registry entry only if it still carries the artifact id of the row they worked from (a conditional replace in
   `FunctionRegistry`, keeping the existing public `replace`/`register` signatures and semantics; the new method may be
   a new public item: list it). On mismatch they abort without writing.
3. No behavioural change for callers that do not race: same errors, same authorization, same write order
   (catalogue first, registry second). Do not hold any DDL lock while invoking user code, and never take the lock on
   the invoke path.
4. A deadlock check by reading: list every lock acquisition and every function that is called while it is held, and
   confirm none of them takes another function lock except rename (ordered).

## Tests (they run in CI, not here)

Deterministic tests with the in-crate `#[cfg(test)]` hook style used by `resolution`/interleaving tests of the previous
task (look in `db/function/invocation.rs` and `tests/catalogue/catalogue_lifecycle_tests.rs`), each its own test:
alter vs a concurrent owner replace (the replace must win and the alter must not roll it back or leave a mismatch);
rename vs concurrent replace of the source; chmod vs replace (no lost metadata); mark_corrupt vs replace (the healthy
new artifact stays); two concurrent replaces end consistent (row id == entry id); rename A->B racing rename B->A does
not deadlock (bounded by a timeout in the test); lock entries do not leak after the operations. Existing tests: never
delete or weaken; list every changed one with the reason.

## Observable acceptance

1. All of the above implemented and tested; expectations come from the rules above.
2. `cargo fmt -p <crate> -- --check`, `cargo check -p <crate> --all-targets` and
   `cargo clippy -p <crate> --all-targets -- -D warnings` clean for every touched crate (`shamir-wasm-host`,
   `shamir-db`).
3. `python -I scripts/ci/check_layout.py` passes (at most 7 entries per folder, 1000 lines per file; new logic in
   new files, not in `management.rs`).
4. rust-guard rules: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no
   visibility widening beyond what is strictly needed (list every one).

## Non-goals

`call_checked`, the language, versions, changes to the invoke/resolution paths, any new locking on read paths,
cross-node coordination.

## Constraints

Compilation, builds and tests run in CI, not here. The only allowed commands are
`cargo fmt -p <crate>`, `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings` and `python -I scripts/ci/check_layout.py`, plus read-only
git. Run cargo only under `wnice -p idle -c 50 -t 8 -m 50 --` and with `-j 2`. No `cargo build/test/run/bench/doc`,
no nextest, no npm. Run `cargo fmt -p <crate>` for every touched crate before the final report.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

Your last message must be a final report (do not end on a tool call): what is implemented, how lock entries are
reclaimed, the lock-order analysis, every widened item, every existing test changed and why, anything unverified.

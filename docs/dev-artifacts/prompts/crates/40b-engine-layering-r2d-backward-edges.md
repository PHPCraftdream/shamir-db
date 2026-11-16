# Brief: R2d revision 1 — six backward edges the layers gate found

Follow-up to `40-engine-layering-r2d-batch-core.md`, same worktree and branch, same rules (no builds or tests, only fmt,
clippy, `cargo check` for the touched crates and their dependents, `check_layout`; no git mutation; final report required;
no function-local `use`, no inline test modules, no new `#[allow]`, no visibility widening beyond what a moved item needs,
at most 7 entries per folder and 1000 lines per file). The R2d change is committed; your edits are working-tree edits on top.

The CI `rust-guard layers` gate (the baseline line `query -> instance` is now removed, so any remaining pair is "new") reports
six production files. Your census grepped `RepoInstance` only; the gate also counts references to items that live under
`repo/instance/` through the `crate::repo::...` re-exports.

1. **`build_reverse_fk_entries` (four query files).** `query/batch/fk/fk_actions/helpers.rs:15`,
   `fk/fk_on_update/scan.rs:14`, `fk/fk_restrict.rs:71`, `runner/query_runner/fk_isolation.rs:121` use
   `crate::repo::build_reverse_fk_entries`, defined in `repo/instance/support/catalog/fk_reverse_cache.rs`. It takes
   `&dyn BatchResolver` (a query-layer trait), so it belongs in the `query` layer: move the function body, unchanged, to a new
   file `query/batch/fk/reverse_fk_scan.rs` (declared in `query/batch/fk/mod.rs`, folder goes from 4 to 5 entries) and point the
   four files and the tests that import it at the new home. Keep the historical path `crate::repo::build_reverse_fk_entries`
   (and `crate::repo::fk_reverse_cache::build_reverse_fk_entries` if something uses it) resolving by a `pub use` of the new
   location from the instance file (instance -> query is the allowed direction); `repo/mod.rs` keeps its re-export line.
   The cache type and `ReverseFkEntry` stay where they are (they are in `table`).
2. **`repo_token` (`tx/commit/core/tx_access.rs:4`).** `begin_tx` on `RepoCore` calls `crate::repo::version_provider::repo_token`,
   which is defined in `repo/instance/version_provider.rs` and is a pure function (a `DefaultHasher` over the name). Move the
   single implementation down to the `repo` layer. `repo/` and `repo/meta/` are full at 7 entries, so no new file there: either
   add it to the existing `repo/meta/repo_identity.rs` (the identity token of a repository, a closely coupled helper) or use an
   existing repo-layer subfolder that has a free slot (`repo/journal/` has 5 entries). Make `tx_access.rs` import it from there, and have `repo/instance/version_provider.rs` re-export it
   so `crate::repo::version_provider::repo_token` and `crate::repo::repo_token` keep working for the other callers and tests.
   Do not duplicate the function.
3. **`StageTimer` (`tx/commit/core/durability_access.rs:27`).** You copied `StageTimer`, `STAGE_WARN_THRESHOLD` and
   `timed_stage` into `durability_access.rs` while the shell file
   `repo/instance/repo_instance/lifecycle/recovery/clean_horizon_shutdown_impl.rs` still defines its own. Two definitions of the
   same type name is a duplicated helper and the gate resolves the `impl Drop for StageTimer` to the instance copy. Keep ONE
   implementation in the lower layer (the `tx` core file, or a small file next to it if the folder has a slot: `tx/commit/core/`
   is at 7 entries, so keep it in `durability_access.rs` unless it would pass 1000 lines), expose `timed_stage` as `pub(crate)`
   through the `core` module's re-exports, and make the shell file import and use it; delete the shell copy of the struct, the
   constant and the function.

After the moves: grep the whole `crates/` tree (not only `shamir-engine`; do not filter paths with `grep -v "^crates/..."`,
Windows prints backslashes) for every moved or re-pointed item, including benches and `shamir-db`, `shamir-sync`,
`shamir-server`; run `cargo check -p shamir-engine -p shamir-db -p shamir-sync -p shamir-server --all-targets`, clippy for
`shamir-engine` and `shamir-db`, fmt and `check_layout`. Re-read every touched file for function-local `use` and for rustdoc
links of the form `[`Self::x`]` that pointed at moved items (replace with plain text). Do not touch the rust-guard files.
In the final report list every item you moved so the delta can be cross-checked, and state which of these three moves you
believe changes a `Self::` or intra-doc link.

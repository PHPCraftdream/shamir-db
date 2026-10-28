BINDING DESIGN (from an @oh read-only review; replaces the Python ratchet):

## Mechanism — clippy, not a text scanner
- DELETE `scripts/ci/gates/unwrap/` entirely (scanner, inventory, tests) and REMOVE every CI step that runs it from `.github/workflows/ci.yml` (fast and full tiers). ci.yml must end up with no reference to engine_unwrap.
- Add `#![deny(clippy::unwrap_used, clippy::expect_used)]` to `crates/shamir-engine/src/lib.rs` (NOT Cargo `[lints]` — the crate uses `[lints] workspace = true`, and `[lints]` would also hit tests/benches).
- Add `allow-unwrap-in-tests = true` and `allow-expect-in-tests = true` to the ROOT `clippy.toml` (a crate-level clippy.toml would replace the root one and lose the disallowed-* rules).
- Every guarded site gets a statement/item-level `#[expect(clippy::unwrap_used, reason = "guard: <one line>")]` (or `expect_used`). rust-guard's structure gate only matches `allow` and is campaign-gated (`.rust-guard/stages.toml` campaigns = []), so `#[expect]` is not blocked; an obsolete `#[expect]` fails as `unfulfilled_lint_expectations` (self-cleaning ratchet).
- Clippy treats only exact `#[cfg(test)]` items as test code. Sites under `cfg(any(test, feature=…))` / `cfg(all(test, feature=…))` and the `#[cfg(test)] if …` blocks (publish_journaled.rs:24-39, test_seam.rs:93-97) may be linted: for cfg(test)-if blocks move the body into a `#[cfg(test)] fn`; for feature-gated test-util code use a non-panicking form (e.g. `unwrap_or_else(PoisonError::into_inner)`) or `#[expect]`. Never put `#[expect]` inside macro bodies (async_stream etc.) — rewrite those sites.

## Site classification (production, 62 incl. order.rs)
| File:line | Class | Action |
|---|---|---|
| format.rs:83,86,90,114,118 | guarded: constant regex literals | #[expect] on each static |
| follower_durability/test_seam.rs:44,49,57,61,77,83,85,98 | guarded: poison impossible | `unwrap_or_else(PoisonError::into_inner)` (as owner.rs:322) |
| test_seams/tx_scan_overlay.rs:212,230 | guarded: peek() is Some | `ov.next_if(..)` or #[expect] |
| table_manager_streaming.rs:851,912 | guarded: recording ⇒ Some | if let / #[expect] |
| interner_manager.rs:154,229 | guarded: OnceCell set, never cleared | #[expect] |
| filtered_vector.rs:133; read_planner.rs:319,448; session.rs:186,665 | guarded: len()==1 | #[expect] |
| session.rs:663 | guarded: any(is_none) → continue | #[expect] |
| fk_isolation.rs:307 | guarded: retry const = 3 | #[expect] |
| read_temporal.rs:247,249,315,317,385; read_index_scan.rs:280,721; collect.rs:143,145,191,213,235,269,313 | guarded: use_topk / has_group_by conjunctions | #[expect] |
| scan.rs:158 | guarded: empty order rejected at :107 | #[expect] |
| vector.rs:111 | guarded: chunk ≤ MAX_TOPK: u32 | #[expect] |
| validator_registry.rs:207 | guarded: RegistryState name↔id invariant under writer lock | #[expect] |
| fk_restrict.rs:283; fk_on_update/scan.rs:159; fk_actions/helpers.rs:188; fk_actions/plan.rs:624 | guarded: every ref_field pre-seeded | #[expect] |
| fk_on_update/plan.rs:141 | guarded: refs filtered by set_fields.contains_key | #[expect] |
| apply.rs:577 | guarded: matches! just above | #[expect] |
| identity.rs:113 | guarded: loop already failed closed | #[expect] |
| read_only_views.rs:232 | guarded: both constructors populate the cell | #[expect] |
| admission/outcome.rs:23 | guarded only by single producer; fields are pub | typed `ok_or_else` error |
| fk_reverse_cache.rs:238 | guarded: every FkSnapshot ctor sets cache | #[expect] |
| tail.rs:61 | guarded: tx_gate() succeeded, OnceCell never reset | #[expect] |
| commit_driver.rs:62 | guarded: Future contract | #[expect] |
| owner.rs:339 | guarded: Open && !spawned && !quarantined | #[expect] |
| owner.rs:494 | guarded: wait_for predicate | #[expect] |
| worker.rs:74,77,85 | guarded: Some until commit/Drop | #[expect] |
| query/read/projection/order.rs (apply_permutation, TopKHeap ids) | guarded: permutation of 0..n; every caller pairs with_ids with Some(id) | REVERT round-2 changes to the original `.expect`s + #[expect] |
| group_execution.rs:38,231 | UNGUARDED (user input) | keep round-2 `DbResult` propagation + its regression tests |
| runner.rs:214 | depends on a `_ => None` wildcard | keep round-2 typed coded error |

Line numbers are from HEAD of the base; re-locate by content. If the lint flags a site not in this table, classify it the same way (guard reason or typed error) and list it in the final message.

## Keep / revert from rounds 1-2
- KEEP: group_execution.rs DbResult change + its two tests; runner.rs:214 typed error; any other replacement only if it is at an input-triggerable site with a test.
- REVERT: every replacement at a guarded site above (restore the original call + #[expect]); order.rs round-2 changes.

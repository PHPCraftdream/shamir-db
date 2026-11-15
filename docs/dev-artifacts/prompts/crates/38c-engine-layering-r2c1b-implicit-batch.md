# Sub-brief 38c of R2c1b — implicit-batch bodies to `query/batch/control/implicit.rs` (ENGINE-LAYERING R2c1b, worker 3)

Read the root CLAUDE.md (binding) and docs/dev-artifacts/prompts/crates/38-engine-layering-r2c1b.md (the parent brief, item 3). You implement worker-3's share: the four implicit-batch convenience methods move their bodies to free functions over `&RepoInstance` in a NEW file `crates/engine/shamir-engine/src/query/batch/control/implicit.rs`; the shell keeps one-line delegating methods (seven production call sites in shamir-db call them by name). Behaviour must not change. You own EXACTLY these files:

NEW:
- crates/engine/shamir-engine/src/query/batch/control/implicit.rs
EDIT:
- crates/engine/shamir-engine/src/query/batch/control/mod.rs (declare `mod implicit;` + re-export the four free fns with `pub(crate) use` — mod.rs stays re-export/decl only)
- crates/engine/shamir-engine/src/repo/instance/repo_instance/transactions.rs

Do NOT touch tx/commit/core/**, catalog*.rs, lifecycle*.rs, or query/batch/mod.rs.

## The four free functions (query layer, over `&RepoInstance`)

Keep each body verbatim (rewrite `self.` → `repo.`), keep the doc comments — especially the compile-time inlining concern (the Box::pin stack-frame explanation on run_implicit_batch_tx). Any `[`x`](Self::x)` intra-doc link is INVALID on a free fn: use plain backticks or a full path that still resolves.

1. `pub(crate) async fn run_implicit_batch_tx<F>(repo: &RepoInstance, actor: Actor, alias: &str, stage: F) -> Result<WriteResult, BatchError> where F: for<'t> FnOnce(&'t mut shamir_tx::TxContext) -> Pin<Box<dyn Future<Output = DbResult<WriteResult>> + Send + 't>>` — body from the shell method (Box::pin'd `repo.begin_implicit_batch_tx` / `repo.commit_implicit_batch_tx` calls, the stage-error mapping to `BatchError::QueryError`).
2. `pub(crate) async fn begin_implicit_batch_tx(repo: &RepoInstance, actor: Actor, alias: &str) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError>` — delegates to `begin_implicit_batch_tx_with_isolation(repo, actor, alias, shamir_tx::IsolationLevel::Snapshot).await`.
3. `pub(crate) async fn begin_implicit_batch_tx_with_isolation(repo: &RepoInstance, actor: Actor, alias: &str, isolation: shamir_tx::IsolationLevel) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError>` — body from the shell method (`repo.begin_tx(isolation)` mapped to `BatchError::QueryError`, `tx.set_actor(actor)`, `tx.set_implicit(true)`). Keep the F-28 Step 5 doc.
4. `pub(crate) async fn commit_implicit_batch_tx(repo: &RepoInstance, tx: shamir_tx::TxContext, alias: &str) -> Result<shamir_query_types::batch::TransactionInfo, BatchError>` — body: `crate::query::batch::receipts::commit_implicit(repo, tx, alias, VisibilityWait::Strict).await`. Keep the doc.

`WriteResult` MUST be imported from the leaf crate directly: `use shamir_engine_query::write::WriteResult;` (this kills the instance-side leaf shadow — parent brief item 3). `Actor` comes from `shamir_types::access::Actor`. This file is QUERY layer: referring to `RepoInstance` (instance layer) is the documented backward pair (baseline `query -> instance`, current count 3 files — this file makes 4, still under the recorded baseline 11; the orchestrator trues it up). Do not import anything else from `crate::repo::instance`.

## transactions.rs edits

- The four shell methods become one-line delegates with IDENTICAL name/signature/visibility/generics/where-clauses, e.g. `crate::query::batch::control::implicit::run_implicit_batch_tx(self, actor, alias, stage).await`. Keep short docs; the long inlining-concern docs moved to the free fns (leave the shell `run_implicit_batch_tx` doc a brief note that the body lives in `query/batch/control/implicit.rs` and why the delegated calls are Box::pin'd).
- `WriteResult` import switches to `use shamir_engine_query::write::WriteResult;` (the signatures still need it).
- Worker 1 simultaneously moves `tx_gate`, `initialized_tx_gate` (its core twin lands on `TxHorizon` + a `RepoCore` one-liner), `max_inflight_commit_version` (DELETED from the shell — its only caller moved), `tx_info_store` onto `RepoCore`; you rewrite the SHELL copies as one-line delegates to `self.core.<method>` (tx_gate, initialized_tx_gate, tx_info_store) and DELETE `max_inflight_commit_version` from the shell entirely. Keep `current_commit_version`, `consistent_version`, `begin_tx`, `commit_tx`, `commit_tx_with_options`, `with_commit_visibility_wait`, `commit_visibility_wait`, `replication_bookmark` UNCHANGED (they call shell methods/delegates that still exist).
- Prune/add header imports: `provenance_gate` (`crate::meta::repl::gate_provenance`) and any now-unused imports (`crate::meta::recovery_marker` if unused, `Repo` trait if unused after tx_info_store stops calling store_get, etc.) must be dropped; `BatchError`, `Actor`, `Pin`, `Future`, `Arc`, `DbResult`, `DbError` (if still used), `VisibilityWait`, `TransactionInfo` path stay as the signatures require. No function-local `use`.

## Hard rules
Same as parent brief: no function-local `use`, no new `#[allow]`, no signature/visibility changes on the shell methods, terse comments, fix stale doc links, re-read cfg-gated code, never run cargo (orchestrator gates), read-only git only, never mutate git state. File under 1000 lines; `query/batch/control/` ends at 5 entries.

## Report (final message)
List the four free fns, the re-exports, every shell delegate/rewrite/deletion in transactions.rs, every import added/pruned, and anything you decided beyond this brief.

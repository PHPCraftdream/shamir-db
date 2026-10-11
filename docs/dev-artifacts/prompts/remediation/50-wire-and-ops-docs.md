# Brief — DOC-1 wire contract, error codes, operator and limitation docs

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.
## FILE SCOPE (only these paths may be edited)

- DOCS ONLY: `docs/guide-docs/client-server-protocol-spec/` (extend an existing file or add at most ONE new file, e.g. a transaction-outcome / error-code reference; update `README.md` of that dir if you add a file)
- `docs/guide-docs/guide/07-operations.md`
- `docs/dev-artifacts/KNOWN_LIMITATIONS.md` - every section EXCEPT the replication journal ordering paragraph (~:491-498): another task rewrites that one later; do not touch it
- NO code, NO tests, NO other docs. Do not edit anything under `crates/`.

## Task

DOC-1 (docs only). Several user-visible behaviours merged on master without operator/client documentation. Derive every fact from the CODE at your base (never from memory); cite the file in a short parenthetical where it helps a reader find the source. Query examples in guides must use the builder (`shamir-query-builder` / the typed TS builder), not raw JSON - raw wire shapes are allowed only inside `client-server-protocol-spec/`.

Document, each in the doc where a reader would look for it:
1. Transaction outcome wire contract. `TransactionInfo` (`crates/shamir-query-types/src/batch/transaction_info.rs`): `status`, `reason`, and the NEW additive `error` field. A commit whose WAL durability failed AFTER the version was published is reported as `status: "committed"` with `error: "wal_durability"` (never as aborted, never retry the writes; the TS `db.tx()` helper surfaces it as an Error carrying `transaction` and `commit_version`, with no rollback). `wal_requires_reopen` (the repository must be reopened; new commits are refused). `materialized` semantics. Read `crates/shamir-engine/src/tx/commit.rs` (`TxError`, `into_transaction_info`, `wal_completion`) and `crates/shamir-db/src/shamir_db/execute/db_tx.rs`.
2. Error-code reference for batch / TxExecute / commit: every code the server maps from `BatchError` and commit errors (read `crates/shamir-server/src/db_handler/handler.rs` `error_code` and `tx_handlers.rs`): which codes drop the interactive handle (time-out, `LimitExceeded` kinds, `tx_too_large`, `tx_expired`), which are retryable (`tx_conflict`, `version_conflict`), which are not (`unique_violation`, `wal_durability`), and `table_migrating` (migration source freeze).
3. Operator guide (07-operations.md): the server-owned limit policy (caps for every limit, `ExecPolicy`, engine ceiling 100 MiB / 1000 queries / 3600 s), `enforce_result_size` (ON in the server, OFF for embedded and WASM `db_gateway`), the runtime byte budget semantics ("bytes materialized by this execution", intermediates and ForEach/nested containers included, no give-back, check before commit), the runtime work counter, the exact TxExecute call budget (remaining handle lifetime at full Duration precision, taken after the context lock), `string_agg` separator and accumulator caps, and the migration source freeze. Keep numbers in sync with `crates/shamir-server/src/config.rs` and `crates/shamir-query-types/src/batch/` constants.
4. KNOWN_LIMITATIONS: transactional reads. Snapshot/Serializable reads are snapshot-pinned scans (no counter / index / min-max / sorted shortcuts inside such a transaction, so large tables pay O(N)); Pessimistic keeps latest-committed reads with locks; non-transactional reads keep all shortcuts; vector similarity inside Snapshot/Serializable still uses the latest committed index plus the staged overlay (not yet snapshot-consistent; tracked as B04b) - write that sentence so it is easy to delete when B04b lands. Also record the remaining honest edges: enforcement of time budgets is cooperative (a stalled operation is not preempted); nested batches inherit the root string_agg accumulator cap and report it as a query error rather than `LimitExceeded`; index-only transactions are outside the writer-drain protocol; the literal-NULL aggregate input limitation ONLY if it is still true at your base (grep `select.rs` aggregate `input` docs: another task may be fixing it).
Verify every claim against the code before writing it; if a claim cannot be verified, leave it out and say so in the report. Do not invent behaviour.

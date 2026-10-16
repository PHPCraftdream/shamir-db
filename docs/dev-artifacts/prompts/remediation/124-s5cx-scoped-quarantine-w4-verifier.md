# S5c-X PR8b-2 W4 — verifier (read-only)

You are the final verifier for S5c-X PR8b-2 (master brief: `docs/dev-artifacts/prompts/remediation/120-s5cx-scoped-quarantine.md`). Stages W1–W3 are in the working tree, uncommitted. You do NOT fix anything: you read and report. The orchestrator will dispatch fixes.

## Rules

- ⛔ NEVER run any git command that mutates the working tree or index (`git status` / `git diff` / `git log` are allowed — read-only only).
- ⛔ NO compile/test/lint commands (`cargo test/build/check/clippy/nextest/rustc/fmt` all forbidden — including `cargo fmt`).
- Read files with your file tools; be exhaustive — this gate replaces compilation for the whole PR.

## Method

1. Run `git status --porcelain` and `git diff --stat` to enumerate every changed/new file.
2. Read EVERY changed/new file IN FULL (not snippets).
3. For each changed public/private item, find and read all DIRECT CALLERS workspace-wide: `grep` across `crates/` including `benches/`, `examples/`, all `tests/` trees, and `shamir-client-ts/` + `shamir-client-node/` for wire/field-level coupling (subscription row fields, error codes).
4. Check, per file:
   - Name resolution: every referenced item exists with that exact path/spelling (read the definition, don't assume); no orphaned imports; no missing imports (especially items used inside new `impl` blocks and test modules).
   - Privacy: `pub` / `pub(crate)` / `pub(super)` correct for every new item; nothing used across module boundaries that isn't visible there.
   - Imports at file tops only; `mod.rs` manifests re-export only; test-only imports gated `#[cfg(test)]`.
   - Signatures at every call site match the new definitions (arity, types, `Option` wrappers, moved/renamed items: `SubscriptionBinding`, `mark_subscription_terminal_generation`, `mark_subscription_resync_required_generation`, `ensure_destinations_unbound`, `remove_db`'s `DbResult<bool>`, `FollowerLoopConfig.binding_generation`, `Subscription.binding_generation/binding_destinations`, the rewritten `ResyncQuarantineCache` API used by `db_tx.rs`/`db_execute.rs`/`registry.rs`/`admin_replication.rs`).
   - `Send` across `.await`: every `tokio::spawn`ed closure captures only `Send` data (new captured `generation`/config fields included); no non-Send guard held across an await (watch for the `clippy::await_holding_lock` class).
   - Clippy `-D warnings` classes, by eye: unused imports/variables, `needless_borrow`, `clone_on_copy`, `int_plus_one`, `redundant_clone`, `single_char_pattern`, `type_complexity` (the two-level Fx map type — consider a type alias), `manual_strip`, `collapsible_if`, unused `mut`. NO `#[allow]` was added anywhere.
   - Layout ratchet: `wc -l` every changed file (≤1000) and count entries of every changed directory (≤7; over-limit dirs must not have grown). Report the measured numbers.
   - Inline-future discipline: no new heavy await inlined into nested-batch/admin-dispatch/ALTER-SUBSCRIPTION/seed-install chains; the synchronous quarantine helpers stayed `#[inline(never)]` and sync.
   - Query-builder rule: no raw `serde_json` query construction introduced.
   - Tests: every new test file wired into its `tests/mod.rs`; test-only seams (`flush` fault injection, task-locals) `#[cfg(test)]`-gated; no test writes machine-specific absolute paths.
5. Trace the six merge criteria of the master brief §6 against the actual diff and name the code + test that satisfies each (or flag the gap).

## Report format

- Verdict per file: OK / issues found (with file:line and the exact problem).
- A consolidated issue list ordered by severity (compile-blockers first, then lint risks, then coverage gaps).
- The measured line counts for every file over 900.
- Explicitly confirm: no global sentinel path remains; no profile resolution on the quarantine read path; prepublish→revoke→persist→refresh ordering intact; open-time validation rejects every legacy shape listed in evidence row 4.

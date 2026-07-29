# Brief for F-65 (#891, P1) — FK indexed-action fast paths must not swallow read errors

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P1-6) found that the F-53c FK indexed CASCADE/SET NULL/ON UPDATE
fast paths collapse three distinct outcomes into one "skip this
candidate": row genuinely absent, storage error, and decode error. After
an AUTHORITATIVE index lookup told the code a candidate exists, a real
read error must abort the RI (referential integrity) operation, not
silently shrink the affected-row set — a silently-shrunk CASCADE/SET NULL
fan-out, or a silently-skipped ON UPDATE propagation, is an RI violation
with no error surfaced to the caller.

This is the SAME defect class F-55 (#881, already landed, commit
`f9eed337`) fixed in the FK reverse-cache discovery scan — `_ =>
continue` where a real error deserves propagation — but in a DIFFERENT
subsystem (the fast-path candidate RE-READ, not the discovery scan).
Read F-55's landed diff first (`git show f9eed337`) as the precedent for
style/reasoning; this task applies the same fix philosophy to new sites.

### The four confirmed sites (orchestrator-verified, read each in full context before touching)

1. **`crates/shamir-engine/src/query/batch/fk_actions.rs:449-453`**
   (direct-child CASCADE/SET NULL index fast path):
   ```rust
   let bytes = match child_table.read_one_tx_bytes(id, Some(tx)).await {
       Ok(Some(b)) => b,
       _ => continue,
   };
   ```
2. **`crates/shamir-engine/src/query/batch/fk_actions.rs:769-774`** — the
   grandchild-recursion sibling of site 1 (comment there says "F-53c:
   index fast-path — same shape as the direct-child scan").
3. **`crates/shamir-engine/src/query/batch/fk_actions.rs:634-638`** —
   inside the grandchild ref-field-value collection loop (reads each
   already-cascade-selected parent/child row to collect its ref_field
   values for further grandchild discovery). Same `Ok(Some(b)) => b, _ =>
   continue` shape. Not labeled "fast path" in a comment, but a genuine
   read error here silently drops that row from grandchild cascade
   consideration — the same class of RI risk. Confirm this reasoning
   yourself by reading the surrounding function before fixing it; if you
   conclude it's actually NOT reachable with a genuine error / not RI
   relevant, say so in your summary rather than silently skipping it.
4. **`crates/shamir-engine/src/query/batch/fk_on_update.rs:459-463`**
   (ON UPDATE index fast path, sibling of sites 1/2 for the `on_update`
   action instead of `on_delete`):
   ```rust
   let bytes = match child_table.read_one_tx_bytes(id, Some(tx)).await {
       Ok(Some(b)) => b,
       _ => continue,
   };
   ```

**Verified NOT in scope:** `fk_restrict.rs:126-129` (`match
parent_values.get(...) { Some(v) if !v.is_empty() => v, _ => continue }`)
looks similar but is NOT this bug — it's a `HashMap::get()` on an
in-memory collected map (no I/O, no error possible), and `_ => continue`
there means "no parent values collected for this field, nothing to
check" — legitimate control flow, not error-swallowing. Do not touch it.

## What to do

1. **Read `read_one_tx_bytes`'s signature and error type** (find its
   definition on `TableManager`) to confirm exactly what `Err` variant(s)
   it can return and what caller context each of the four sites is in
   (what `Result<_, E>` the enclosing function returns), so your fix
   compiles cleanly.
2. **Fix each of the four sites** to distinguish the three outcomes:
   - `Ok(Some(b))` → `b` (unchanged, the row exists, use it).
   - `Ok(None)` → the row is genuinely absent (correctly excluded) — this
     alone should `continue`.
   - `Err(e)` → a real storage/decode error. This must ABORT the whole RI
     operation with an error, not silently shrink the candidate set.
     Propagate it as the enclosing function's error type — check how
     nearby code in the SAME function already constructs `BatchError`
     (e.g. the `index_candidate_ids` call a few lines above each site
     already does `.map_err(|e| BatchError::QueryError { alias:
     alias.to_string(), message: format!("fk_actions: ...: {e}"), code:
     Some("fk_actions".to_string()) })?` — mirror that exact shape for
     consistency, with a message identifying which site/operation
     failed).
3. **Add fault-injection tests** proving a storage/decode error during
   the fast-path candidate re-read now aborts the RI operation (returns
   `Err`) instead of silently reducing the affected-row set. `TableResolver`
   test doubles (like F-55's `PoisoningResolver`) won't directly help
   here since `read_one_tx_bytes` is a `TableManager` method, not
   something a `TableResolver` wraps — investigate the cleanest way to
   inject a failure at this exact call:
   - Check whether a `#[cfg(test)]` pause/failure-injection seam already
     exists anywhere near `read_one_tx_bytes` or similar low-level
     table-read paths (this codebase has several such hooks — e.g.
     `TEST_POST_BARRIER_PRE_WRITE_HOOK`, `TEST_POST_GENCHECK_PRE_PUBLISH_HOOK`,
     `TEST_SEEK_LOOP_PRE_ITER_HOOK` — search for the pattern) that could
     be adapted or extended.
   - If no clean injection point exists, consider whether corrupting the
     underlying storage bytes for a specific `RecordId` (so `read_one_tx_bytes`
     hits a genuine decode error, `Err`, not `Ok(None)`) is a viable,
     deterministic way to trigger the `Err` branch without a new test
     hook — this may be simpler than building new instrumentation.
   - Pick whichever approach is cleanest and most deterministic (no
     sleeps/timing races) given what you find; explain your choice in
     the summary.
   - At minimum, cover site 1 and site 4 (the two clearest, most
     independent CASCADE/SET NULL and ON UPDATE fast paths) with a real
     fault-injection test each. Cover sites 2/3 too if the same test
     harness extends cleanly; if not, explain why and what coverage gap
     remains.

## What NOT to do

- Do NOT touch `fk_restrict.rs:126-129` — confirmed not a bug (see above).
- Do NOT touch the full-scan fallback paths (`classify_row`/
  `classify_row_update`'s callers in the `else` branches) — those already
  use `list_stream_tx`, a different, already-correct error-handling shape;
  this task is scoped to the FOUR index-fast-path candidate re-read sites
  above.
- Do NOT touch F-55/F-56/F-57/F-58/F-59/F-60/F-61/F-63 (other
  already-landed tasks from the same review) or F-66/F-67 (other pending
  tasks).
- Do NOT change `index_candidate_ids`'s own error handling — it already
  propagates correctly (`.map_err(...)?` immediately above each fast-path
  branch); this task is about the RE-READ loop that runs AFTER
  `index_candidate_ids` already succeeded.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- TDD: write the fault-injection test(s) first, confirm they fail against
  the current (buggy) code, then make the minimal fix, confirm green.
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

Plus a personal red-then-green reproduction of at least one fixed site.

When done, give your final summary as plain text: the exact diff for all
four sites (or explain if any were found not to need a fix), the
fault-injection test strategy chosen and why, which sites got dedicated
tests, and confirmation fmt/clippy/tests are clean.

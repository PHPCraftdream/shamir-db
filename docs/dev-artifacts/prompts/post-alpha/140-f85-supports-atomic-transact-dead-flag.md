# F-85 (#913) — supports_atomic_transact() is unread dead metadata

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

This is a finding from an `@oh` adversarial review of the F-69..F-81
remediation wave (see `docs/checkpoints/p0-p1-wave-complete.md`, task
#913). F-77 (#904, commit `cff22d54`) added
`Store::supports_atomic_transact(&self) -> bool` to
`crates/shamir-storage/src/types.rs` (default `false`; `FjallStore` →
`true`; `CachedStore`/`MemBufferStore` delegate to `inner`; `MirroredStore`
→ `false`, explicit), with a doc comment stating: "Callers that need true
cross-op visibility atomicity MUST check `supports_atomic_transact`."

A workspace-wide check found **zero production readers** of this method —
the only callers are assertions in
`crates/shamir-storage/src/tests/storage_mirrored_tests.rs`. The doc
promises a check that nothing performs.

## The residual it was meant to make inspectable

F-77's own audit (see its brief,
`docs/dev-artifacts/prompts/post-alpha/133-f77-mirrored-store-transact-atomicity.md`,
and commit `cff22d54`'s message) found four production `transact` call
sites that route through `MirroredStore` (which returns `false`) writing
ephemeral posting keys:

- `crates/shamir-index/src/write_ops.rs:46` (`apply_index_ops`)
- `crates/shamir-index/src/write_ops.rs:108` (`apply_index_ops_at_commit`,
  Phase 5c)
- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` —
  `rekey_sorted_prefix` (check current line number, may have shifted since
  F-77/F-78/F-81 touched this file)
- `crates/shamir-index/src/legacy/index_manager.rs` — `apply_ops` (check
  current line number)

Concrete window at `rekey_sorted_prefix`: it issues a batch of
`[Set(new_id/k), Remove(old_id/k)] × N`. A concurrent `scan_prefix_stream`
reader on `MirroredStore`'s lock-free primary between per-op applications
can observe key `k` under BOTH ids (double-count) or, mid-`Remove`, under
NEITHER — self-healing via the existing settle/re-scan loop (per F-77's
audit — not a persistent corruption risk), but currently NOTHING gates on
`supports_atomic_transact()` to even acknowledge this is a known, accepted,
self-healing gap at the point where it actually matters.

## What to decide (read F-77's brief and commit first, then choose)

Pick ONE of these two resolutions — do not attempt both, and do not
over-engineer:

**Option A — downgrade the doc (default, prefer this unless investigation
finds a concrete reason for Option B).** Reword
`Store::transact`'s / `supports_atomic_transact`'s doc in
`crates/shamir-storage/src/types.rs` from "Callers that need true cross-op
visibility atomicity **MUST** check `supports_atomic_transact`" to
something like "Callers whose correctness DEPENDS on cross-op visibility
atomicity should check `supports_atomic_transact` before assuming it;
today's production callers (rekey_sorted_prefix, apply_index_ops,
apply_index_ops_at_commit, legacy apply_ops) all tolerate the non-atomic
case via a self-healing settle/re-scan mechanism, so none of them
currently need to." This keeps the flag honest, queryable metadata without
implying a check that doesn't exist anywhere. Cross-reference this
resolution from `MirroredStore`'s own doc comment too, so a reader
encountering either side of the story gets the same accurate picture.

**Option B — add one real caller.** Only choose this if, while
investigating the 4 call sites above, you find one where a debug-time
sanity signal is genuinely valuable (e.g. `rekey_sorted_prefix` gaining a
`debug_assert!` or a `log::trace!` that fires when
`!store.supports_atomic_transact()`, documenting "this operation is
running against a non-atomic backend, relying on the settle-loop
self-heal" — cheap, off the hot path in release builds if using
`debug_assert!`/`cfg(debug_assertions)`). Do NOT add runtime behavior
changes (no new fallback logic, no blocking, no retry loop) — this task is
about making the flag's existing honesty complete, not about hardening the
atomicity guarantee itself (that would be a much larger, separately-scoped
change per F-77's own conclusion that the current gap is acceptable).

Whichever you choose, state your reasoning in the commit message
explicitly, including why the other option was rejected.

## Definition of done

- Either the doc is corrected to not overclaim a check nothing performs
  (Option A), or exactly one real caller reads the flag with a narrow,
  debug-only/logging-only signal (Option B) — not both, not neither.
- No behavior change to production atomicity/correctness — this is a
  documentation-accuracy or observability task, not a hardening task.
- `cargo fmt -p shamir-storage -p shamir-index -p shamir-engine -- --check`
  and `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-storage -p shamir-index -p shamir-engine --full`
  green.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.

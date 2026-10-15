# Round 11 — fail-closed quarantine marking (S5c-X, CI 7161 follow-up)

## Failure

CI: 7161 pass, 1 fail — `replication_ddl_tests.rs:272`
`mark_subscription_resync_required` unwrapped
`Err(Validation("quarantined profile resolution failed: seed_wrong_scope"))`.
The existing test creates a subscription whose profile row does not exist;
marking the quarantine aborted, so the safety state was never persisted.

## Required semantics (user-stated)

Quarantine is a safety action: marking must NEVER fail because the covered
set cannot be computed. Persisted `resync_required` state + durable
`quarantined` marker are written REGARDLESS. If the profile / covered repos
cannot be resolved, the read gate fails CLOSED over the WIDEST SAFE SCOPE;
the resolution failure is `log::warn!`-ed and surfaced via the row's
`status_message`, never returned as a marking error.

## Fallback scope choice + justification

Row shape inspection (`handle_create_subscription`, admin_replication.rs):
subscription rows persist only `name`, `upstream`, `publication`, `profile`,
`state` (+ later `seed_receipt` / `resync_from_receipt` / `quarantined` /
`last_terminal_reason` / `status_message`). There is NO `db` column, and
`ShamirDb` is multi-db (`dbs: DashMap`) with no single "own database".
Therefore no `(db, "*")` key can be derived for the row — the widest scope
that is GUARANTEED to cover the unresolvable profile's targets is the global
sentinel `("*", "*")`, already supported by `ResyncQuarantineCache::reason_for`
(consulted after the `(db, repo)` and `(db, "*")` lookups). A db named `*` is
not constructible, so the sentinel cannot collide with a narrower entry.
Quarantining every known db via `list_dbs()` was rejected: dbs created after
the fallback would be missed — the sentinel is strictly safer and O(1).

## Changes

1. `execute/admin_replication.rs`
   - `mark_subscription_terminal` (`resync_required` branch, ~:892-908):
     removed the two `ok_or_else`/`streams?` abort paths; resolution now
     goes through the shared `quarantine_row_entries` helper — on success
     the normal per-stream `(db, repo)` entries are pushed (unchanged
     path); on missing/unresolvable profile a sentinel entry is pushed, the
     failure is `log::warn!`-ed, and it is appended to the persisted
     `status_message`. The persisted row write always happens.
   - `resync_quarantine_entries` (~:752-822): same per-row fail-closed
     fallback via the shared helper instead of `?` on the row. Infra-level
     failures of the `subscriptions` table read itself still propagate via
     `?` (pre-existing behavior); only per-row profile resolution degrades
     to the sentinel. Missing-profile rows fall back the same way.
   - New private helper `quarantine_row_entries` (bottom of file): shared
     per-row resolution + sentinel fallback, returns `(entries, failure)`;
     dedupes the two call sites and keeps the file within the 1000-line
     ratchet (exactly 1000).
   - Why-comments on both functions document the fail-closed semantics.
2. `execute/repo_admin/quarantine_cache.rs` (pre-existing in tree from the
   concurrent round-10 writer, verified): `global_sentinel_key()` /
   `unresolvable_profile_reason()` helpers + `reason_for` sentinel lookup.
3. Tests:
   - `tests/replication_ddl_tests.rs`: new
     `resync_marking_with_unresolvable_profile_fails_closed_globally` —
     marking with a never-created profile succeeds; state = `resync_required`
     + durable `quarantined` marker persisted; `status_message` names the
     profile resolution failure; `read_changelog_bounded` refused with code
     `resync_required` for `(testdb, any_repo)` AND `(otherdb, other)`
     (sentinel scope); list/status echo the quarantined state + message.
     The pre-existing test (which uses no profile) is UNTOUCHED — after the
     fix its marking succeeds and its assertions still hold (sentinel
     quarantine refuses its admin actions identically).
   - `gate_tests.rs`: an extra unresolvable-profile test was found already
     in the tree (authored by the concurrent round-10 writer); left as-is.

## Constraints honored

Builder-only queries; no new files; no deps/config; NO cargo/git commands
run (parent runs scoped fmt + commit). Honest uncertainty: no compile/test
claims — code verified by inspection only (signatures of
`catalogue::streams(&ShamirDb, &str) -> Result<Vec<ReplStream>, SeedRefusal>`,
`DbError::code()`, `seed_status_entries` `!emitted → push(record)` fallback
all confirmed by reading). Note: a concurrent writer was active on this
worktree at session start (mtimes 12:45-12:46); all edits were made against
fresh reads after it went quiet.

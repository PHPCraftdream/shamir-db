# Checkpoint — 2026-07-29 [review-remediation-wave]

## Session summary

This session continued from `docs/checkpoints/2026-07-29-f53-wave-complete.md`.
An independent readonly review of snapshot `e145b1d3`
(`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`, produced
by a separate process/session, not authored here) found 5 correctness P0s and
2 release-engineering P0s, plus several P1s, and gave a NO-GO verdict for the
first public alpha tag. I read the review, independently re-verified its key
factual claims (fail-open FK discovery code, missing `bench-baseline.json`,
missing git tag despite crate version already at `0.1.0-alpha.1`, mutable
`dtolnay/rust-toolchain@1.93.0` refs, missing perf-gate in release DAG — all
confirmed true), wrote a remediation plan
(`docs/dev-artifacts/roadmap/2026-07-29-pre-alpha-remediation.md`), and
decomposed it into TaskList items #881–#893 across three waves (P0 correctness,
release-consistency, P1 debt). A fourth "wave" (breadth: DDL/OQL/builders, and
several P1 items) was deliberately NOT ticketed — explained in the plan's §5
as premature until the P0 wave lands.

**Wave 1 (P0, all 7 tasks) is fully done, verified, and committed**:
F-55 (`f9eed337`) fail-closed FK reverse-cache discovery; F-56 (`7fde958e`)
WriterDrainBarrier SeqCst memory-ordering proof + loom model + wired
`drain_writers()` into `create_index_v2`; F-57 (`fcaae001`) unified online
CREATE INDEX lifecycle across regular/unique/sorted index families; F-58
(`15b5a729`) closed the AsOf index-seek TOCTOU race with a post-scan
high-water re-check; F-59 (`a66d68d6`) fixed `MirroredStore::transact`'s
whole-batch error atomicity (reordered so the mirror commit runs before
EITHER subset touches primary); F-60 (`a5c3a29d`) hardened
`scripts/bench_gate.sh`'s parser to fail closed on missing/duplicate/
unbacked-baseline cells; F-61 (`015e9b5f`) restricted `perf-gate.yml` to
`workflow_dispatch` only after discovering — via `gh repo view` — that the
repo is ALREADY PUBLIC (not a future concern), so the old `pull_request`
trigger was an ACTIVE untrusted-code-execution risk on the persistent
self-hosted runner. Every wave-1 fix got a personal zero-trust
verification (full diff read + fmt/clippy/test re-run + a genuine
red-then-green reproduction by temporarily reverting the exact mechanism).

**Wave 2 (release-consistency)**: F-62 (`8eaf16cf`) wired perf-gate into
`release.yml`'s DAG (a new inline `perf-gate` job, duplicating the recipe —
cross-workflow `needs:` isn't supported — added to every downstream job's
`needs:`; documented that a real tag push will now queue forever until an
operator registers the `shamir-bench` runner, which is intentional). F-63
(`75c156ba`) pinned `dtolnay/rust-toolchain@1.93.0` to its resolved commit SHA
(`d0befba8b9ddf874327619e84c39b094edd58b66`) across all 21 occurrences in 7
workflow files; noted an open Dependabot PR (#11) will likely need
regeneration against the new SHA-pinned format. **F-64 (version/CHANGELOG/git
tag reconciliation) was explicitly DELETED by the user** ("удали вообще
задачу в сервиями и тегами, мы не занимаемся этим никак сейчас" / "мы
занимаемся только кодом" — we only deal with code, not release/versioning
process) — this is a firm scope boundary for the rest of the session, not
just this task.

**Wave 3 (P1 debt) is IN PROGRESS**: F-65 (#891, FK indexed-action fast-path
read-error swallowing) is the current in-flight task — see "Currently
unresolved" below. F-66 (#892) and F-67 (#893, blocked by F-58/#884 which is
done) have not started.

**Crush provider hit a hard weekly/monthly quota limit mid-F-65**
("too many requests: Weekly/Monthly Limit Exhausted... resets 2026-07-30
17:01:11"). Per the user's earlier `/crush-fallback @sh` arming, this
triggered the hard-limit branch: the `crush-fallback state (persistent — do
not complete)` marker task (#894) is `STATUS: active, AGENT: sh,
TRIGGER: hard-limit, CRON_JOB_ID: none` (no cron armed for this trigger
type). All delegated work from this point (F-65 onward) is routed through
`Agent({subagent_type: "sh"})`, not `crush run`, until `/crush-fallback clear`.
A `/babygoal доделай задачи c помощью @sh` was issued mid-session, setting an
active Stop-hook goal with that exact text; the babygoal contract's own
checks (recurring babysit cron present, TaskList already correctly
decomposed as sequential leaf tasks) were confirmed already satisfied without
needing new setup.

## Currently unresolved — F-65 (#891)

The sh sub-agent (agentId `a4f7f2ad94002105c`) fixed all four confirmed
`_ => continue` → proper `Ok(None)`/`Err(e)` sites (`fk_actions.rs` x3,
`fk_on_update.rs` x1 — diffs read and confirmed correct by me), added a new
`#[cfg(test)]` failure-injection seam (`TEST_READ_ONE_TX_BYTES_FAILURE` /
`ReadOneTxBytesFailHook` in `table_manager_streaming.rs`, re-exported from
`table/mod.rs`), and wrote 3 new tests in the new file
`fk_indexed_action_read_error_tests.rs`. The agent's OWN self-report said it
had NOT yet run the full gate. I ran it myself (zero-trust, not trusting the
partial self-report): `cargo fmt -p shamir-engine -- --check` initially
FAILED (unformatted spots in the new test file — I ran `cargo fmt -p
shamir-engine` myself to fix this cosmetic issue), clippy is clean, but
`./scripts/test.sh -p shamir-engine --full` found a GENUINE failure:
`on_update_index_fast_path_propagates_read_error` panicked because the
update actually went through the FULL-SCAN fallback (`index_used: None,
records_scanned: 1` in the result), not the F-53c index fast-path the test
claims to exercise — meaning the injected failure hook (keyed to the
fast-path's `read_one_tx_bytes` re-read call) never fired, and the test
would have ALSO passed silently on the pre-fix buggy code. I sent a detailed
message back to the sh agent (via `SendMessage` to `a4f7f2ad94002105c`)
explaining the exact diagnosis and asking it to: investigate
`plan_fk_on_update`'s real fast-path-eligibility conditions (compare against
an existing passing fast-path test in `fk_on_update_tests.rs`), fix the
TEST setup (not the production code, which is already correct) so the fast
path genuinely engages, and re-run the COMPLETE gate itself afterward. No
response/notification had arrived by the time this checkpoint was written —
**this is the single open item blocking F-65's completion.** The other 3
new tests (`cascade_index_fast_path_propagates_read_error`,
`cascade_grandchild_recursion_propagates_read_error`) were NOT reported as
failing in the same full-suite run, so they are presumed passing, but I have
not independently re-confirmed that after the fmt fix — worth a quick
re-check once F-65's failing test is fixed, as part of the final full-gate
re-run anyway.

## Active goal

`доделай задачи c помощью @sh` (finish the tasks using @sh) — a session-scoped
Stop hook, set via `/babygoal` mid-session. This governs continuing F-65
through F-67 via the `sh` Agent subtype (per the active crush-fallback
hard-limit state) rather than `crush run`.

## TaskList

### in_progress
- #891 F-65 (P1): FK indexed-action fast path must not swallow read errors — blocked on one failing test (see "Currently unresolved" above), owned by sh sub-agent `a4f7f2ad94002105c`

### pending
- #892 F-66 (P1): remove `std::sync::Mutex` from `TxContext::ri_barrier_tokens`
- #893 F-67 (P1): per-index mutation epoch instead of manager-wide high-water (blockedBy: none listed now — #884/F-58 it depended on is completed)
- #894 crush-fallback state (persistent sentinel — do not complete; currently STATUS: active, AGENT: sh, TRIGGER: hard-limit)

### recently completed (most recent 10)
- #889 F-63 (P1-R4): pin `dtolnay/rust-toolchain` to immutable commit SHA — `75c156ba`
- #888 F-62 (P1-R3): wire perf-gate into the release DAG — `8eaf16cf`
- #887 F-61 (P0-R2): restrict `perf-gate.yml` to `workflow_dispatch` (repo confirmed already public) — `015e9b5f`
- #886 F-60 (P0-R1): perf-gate fail-closed bench parser hardening — `a5c3a29d`
- #885 F-59 (P0-5): `MirroredStore::transact` whole-batch error atomicity — `a66d68d6`
- #884 F-58 (P0-4): AsOf index-seek TOCTOU race closed — `15b5a729`
- #883 F-57 (P0-3): unified online CREATE INDEX lifecycle — `fcaae001`
- #882 F-56 (P0-2): WriterDrainBarrier SeqCst proof + loom — `7fde958e`
- #881 F-55 (P0-1): fail-closed FK reverse-cache discovery — `f9eed337`
- (F-64, version/tag reconciliation — DELETED by explicit user decision, not completed)

All prior-session tasks (#857–#880, the F-46 through F-54 wave) are also
`completed` from before this checkpoint's window.

## Decisions

- Independent review's findings were NOT taken on faith — every P0's core
  factual claim was personally re-derived/re-verified from the actual code
  before a brief was written (e.g. re-deriving the exact weak-memory proof
  for F-56, confirming `MirroredStore.primary`'s concrete `InMemoryStore`
  type for F-59, confirming the repo's ACTUAL public visibility via `gh repo
  view` for F-61 rather than assuming the review's "not yet public" framing).
- F-64 (version/tag reconciliation) was explicitly cancelled by the user, who
  drew a firm scope line: this session works on CODE only, not
  release-process/versioning bureaucracy. Do not resurrect this task without
  the user raising it again.
- F-61: user chose the `workflow_dispatch`-only restriction (simplest,
  safest for right now) over maintainer-approval-gated trusted workflow or
  an ephemeral runner — those remain documented as possible future work, not
  implemented.
- Crush hard-limit → fallback to `Agent(sh)`, per the user's own
  `/crush-fallback @sh` arming from earlier in the session; this is now the
  active delegation transport until `/crush-fallback clear` or the quota
  resets (2026-07-30 17:01:11, informational only, no cron armed for this
  trigger type per the skill's hard-limit branch).
- Zero-trust verification caught a real gap in F-65's own test suite
  (a test not actually exercising the code path it claimed to) — sent back
  to the delegate for a real fix rather than accepting the partial
  self-report or patching it myself blind.

## Open questions

- None requiring the user right now. The one open item (F-65's failing
  test) is an in-flight delegate-fix loop, not a decision needing user
  input.

## Repo state

```
 M crates/shamir-engine/src/query/batch/fk_actions.rs
 M crates/shamir-engine/src/query/batch/tests/mod.rs
 M crates/shamir-engine/src/table/mod.rs
 M crates/shamir-engine/src/table/table_manager_streaming.rs
?? crates/shamir-engine/src/query/batch/tests/fk_indexed_action_read_error_tests.rs
?? docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md
?? docs/dev-artifacts/roadmap/2026-07-29-pre-alpha-remediation.md
```
(fk_on_update.rs is also modified — `git diff --stat` confirms it, +11/-4 —
though it did not show in one `git status --short` listing snapshot taken
moments earlier during this checkpoint's own preparation, likely a
transient shell/timing artifact, not a real revert; re-verified present via
direct `grep` immediately after.)

```
db908a28 docs(prompts): brief for F-65 -- FK indexed-action fast path read errors (#891)
75c156ba security(ci): F-63 -- pin dtolnay/rust-toolchain to an immutable commit SHA (#889)
8eaf16cf chore(ci): F-62 -- wire the performance gate into the release DAG (#888)
6a6a32f6 docs(prompts): brief for F-62 -- wire perf-gate into release DAG (#888)
015e9b5f security(ci): F-61 -- restrict perf-gate.yml to workflow_dispatch, no auto-run on PRs (#887)
```

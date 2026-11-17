# 34 — R2a 1c wiring tests (release brief)

## Goal

The commit admission gate primitive (`CommitAdmissionGate`, under
`crates/engine/shamir-engine/src/tx/commit/pipeline/admission/`) and its wiring
into the leader commit path (`commit_tx_inner`) and the follower apply path
(`apply_replicated_batch`) are already implemented and accepted. This task
adds the *wiring-level* tests that prove the gate actually behaves as the
design (`docs/dev-artifacts/design/online-backup.md` section 2) demands when
driven through a REAL repository instance — not just through the bare
primitive unit tests already present in `admission/tests/`.

## The four items

1. **`close_refuses_overlay_only_leader`** — with a real `RepoInstance`
   (in-memory repo, existing fixtures), close the admission gate and start a
   leader commit whose ONLY effect is an interner overlay merge (no table
   writes). The commit must be refused (typed `TxError::CommitAdmissionRefused`
   after the 15s `ADMISSION_REOPEN_WAIT_CAP`; the commit entry does not allow
   driving the cap down) BEFORE any side effect, proven by observable state:
   base interner snapshot (len/generation/max_published_id) unchanged, repo
   version (`version_allocation_high_water_mark`) not advanced, WAL position
   (`wal.recover().len()`) not advanced. Negative control: with the gate open
   but the table's migration freeze held, the SAME overlay-only commit
   proceeds — table freezes alone do not stop it, the admission gate does.

2. **`follower_apply_respects_gate`** — while the gate is closed,
   `apply_replicated_batch` refuses immediately with
   `DbError::Conflict("commit admission closed; follower apply refused")`;
   after the close owner drops (reopen) the same batch applies normally.
   Reuses the existing replication-apply fixtures (real `project_event`
   ChangelogEvent, in-memory follower, `ApplyPolicy::Trusted`).

3. **`lease_survives_detached_and_async_tails`** — the gate's ACTIVE count
   stays ABOVE ZERO across (a) a detached `CommitDriver` continuation (the
   suspended-future transfer after `caller.abort()`), (b) the AsyncIndex
   materialization tail until it finishes, and (c) the lock-free commit
   branch inline — then returns to zero. Deterministic handshakes via the
   existing test hooks `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK` and
   `TEST_ASYNC_INDEX_TAIL_HOOK` (park the hook, observe the count, release).

4. **Fix `close_vs_acquire_boundary_never_leaves_an_uncounted_lease`** —
   make the acquirers REALLY concurrent: spawned tasks released by a
   `tokio::sync::Barrier`, each retrying `try_acquire` until refused, every
   recorded outcome asserted to be exactly counted-before-close (`Ok`,
   lease dropped) or refused (`Err(Closed)`), never anything else, with the
   closed-and-zero invariant held while the close owner is alive.

## Policy

- Compilation and tests run on GitHub CI only. Locally allowed: `cargo check`
  / `cargo clippy -D warnings` / `cargo fmt --check` per crate, and the
  layout gate (`python -I scripts/ci/check_layout.py`), one cargo command at
  a time via `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`.
- Deterministic handshakes only — armed notifies and poll-based pending
  checks; NO sleeps as correctness evidence. The only real-time wait in the
  suite is the sanctioned refusal-after-cap wait of item 1 (the commit entry
  fixes the cap as the `ADMISSION_REOPEN_WAIT_CAP` constant).
- No new `#[allow]`, no local `use` inside functions, imports at file top,
  no inline `#[cfg(test)] mod tests` blocks, no logic in `mod.rs`. Layout:
  ≤7 entries per folder, ≤1000 lines per file.
- The gate primitive and its wiring semantics are NOT modified; only the
  existing `#[cfg(test)]` observation surface (`close`, `reopen`,
  `active_count`, `is_closed`) and the existing `pub(crate)` accessor
  `RepoCore::commit_admission()` are used — no new accessors were needed.
- No git mutation; the orchestrator commits.

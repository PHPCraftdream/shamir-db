# Brief — Round 8 quarantine cache publication and revocable facade receivers

## Hard constraints

- Worktree: `D:/dev/rust/shamir-db/worktrees/s5cx-quarantine-reads`.
- Read `CLAUDE.md`, `AGENTS.md`, the Rust-intel skill, and relevant skill modules before Rust edits.
- Do not run builds, tests, clippy, rustc, nextest, or any Cargo command. The orchestrator formats as the final step.
- No git writes or commits. Read-only git inspection only. Never run reset, checkout, clean, stash, restore, rm, or other mutating git commands.
- Make changes only through edit/write. Before code, save this standalone brief at `docs/dev-artifacts/prompts/remediation/108-round8-quarantine-publication.md` (parent verified).
- Scope: shamir-db quarantine cache/publication, facade receiver lifecycle, required callers/adaptations and tests. Engine/tx only if essential for revocation.
- Do NOT edit server bridge/push, replication handler/in_process, or server tests yet. Report changed receiver API/interface for next stage.

## Goals

1. Install quarantine read-gate before persistence/ack of resync transition. It stays fail-closed for every catalogue/profile resolution/persistence failure. Existing mutation control lock serializes writers; readers do not hold it. The current `db_execute.rs` `ResyncQuarantineCache` is DashMap+count with misleading lock-free/atomic documentation. Prefer a verified existing lock-free primitive, no new dependency, one global healthy atomic load with no allocations, and no hot locks. Inspect actual pinned dependency versions in Cargo.lock. Repair swallowed refresh errors and profile-error paths which skip covering repos; inspect initialization behavior.
2. `subscribe_changelog` must stop returning a raw Tokio broadcast Receiver with acquisition-only gating. Already-attached facade receivers must become terminal at the quarantine transition, even when idle; no queued or new data may escape after transition. Transition must notify/close live registry entries for covered repos. Raw Tokio receivers cannot be individually closed externally: use a revocable wrapper/registered lifecycle or equivalent sound design, bounded ownership/cleanup, and recv/try_recv error behavior compatible with actual callers. Sweep all callers and typed receiver signatures, including JournalDrain and tests/benches/client crates. Preserve internal engine raw feed only if facade wrapper reliably closes before data. Prove attach/transition race; do not spawn forwarding tasks that leak.

## Plan and tests

Design before implementing; report cross-stage adaptations. Read actual definitions and reference sites before shared-code changes. Add deterministic regression tests with handshakes and no sleeps:
- attached receiver closes at quarantine flip without a post-transition write;
- queued event cannot escape;
- healthy and unrelated repos remain unaffected;
- cache-before-persist failure if a seam is needed;
- real persisted quarantine path.
Tests are for CI only and must not be run. Explain counterfactual failure statically.

Layout ratchet: at most 1000 lines/file and 7 direct entries/folder; existing over-limit files/folders must not grow vs origin/master. Avoid broad refactors; new topical file/subdirectory only when needed. Imports at top, query builder, no raw JSON.

## Final report

Give detailed diff summary, exact changed receiver interface, all files, static proof of transition ordering and race exclusion, caller adaptations for next stage, and remaining risks. State explicitly that tests/builds were not run and explain statically what regressions would catch. No commit.

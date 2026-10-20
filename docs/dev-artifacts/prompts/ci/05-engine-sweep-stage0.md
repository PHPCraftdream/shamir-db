# CI-04 stage 0: shamir-engine sweep manifest (docs only)

Goal: produce the committed, exhaustive source-to-destination manifest that `05a-engine-sweep-plan.md` names as the prerequisite for authorizing any engine sweep stage. No Rust, Cargo, workflow or baseline file changes in this stage.

## Inputs
- `05a-engine-sweep-plan.md` (adjudicated plan: stage order A1..F, gates, risks, privacy findings).
- `scripts/ci/layout_baseline.txt` (the engine entries), `scripts/ci/check_layout.py` (limits: <= 7 direct entries per folder, <= 1000 lines per file, tracked paths only).
- The engine tree at the current master (`crates/shamir-engine`), which already contains #189 and the CI-03 guards (`tools/rust-guard`, `.rust-guard/stages.toml`).

## Deliverable
`docs/dev-artifacts/prompts/ci/05b-engine-manifest.md` (split into `05b-1..N` parts if a part would exceed 1000 lines, plus the index in `05b`). Every table is produced by a script reading `git ls-files` / the source (not typed by hand), and the generating script is kept as `scripts/ci/gates/engine_manifest_check.py` only if it is needed to re-verify the tables (otherwise keep it in `.scratch`).

1. Inventory: every folder over 7 direct entries and every file over 1000 lines in `crates/shamir-engine` (must equal the engine entries of the baseline).
2. Oversized files: for each, the proposed split into named topic files (destination path, the items/line ranges that move, expected size) plus the privacy analysis (private fields, `pub(super)` meaning after the move, `#[path]` mounts, impl blocks on `RepoInstance` and other types whose private fields the pieces touch). Splits keep private-field access through descendant modules, never widen visibility.
3. Over-full folders: for each, a grouping into <= 7 subfolders with an explicit source -> destination row for EVERY tracked file (no ranges, no "etc."), the new `mod.rs` manifests, and the logical module path of every moved module (so facade re-exports keep old paths resolving).
4. Module graph facts: all `#[path]` mounts in the engine, every `impl RepoInstance` site, every `pub(super)`/`pub(in ...)` item inside a module that moves, downstream consumers of moved test hooks (`shamir-db`, `shamir-server`, benches).
5. Stage assignment: assign EVERY item of 1-3 to one stage of the plan (A1, A2, B1, B2, C1, C2, D1, D2, D3, E1, E2, F) or to two additional parallel stages the plan allows after the facade decision: Q (query/ + validator/ + benches + engine integration tests) and T (table/). State per stage: scope, dependencies, which stages can run in parallel without touching the same files, the exact `.rust-guard` campaign (name, packages, delta file skeleton) it needs, acceptance criteria and non-goals, and what must be reviewed manually (lock/guard lifetime, drop order, cancellation, durability order).
6. Owner questions (at most nine), each with a recommendation.

## Acceptance
- A mechanical completeness check (run by the orchestrator, no build): every engine baseline entry appears in the manifest; every tracked file in an over-full folder appears in exactly one mapping row; every destination folder has <= 7 entries and every destination file <= 1000 lines by the manifest's own numbers.
- No private machine path in any committed text.
- Facts are marked OBSERVED (verified against the tree, with file:line) or DECIDED/INFERRED; no invented counts.

## Non-goals
Moving any file, editing any Rust source, changing the baseline, the workflows, or the guards.

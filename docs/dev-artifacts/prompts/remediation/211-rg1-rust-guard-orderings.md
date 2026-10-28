# Stage RG1 — rust-guard `orderings`: AST pin gate for atomic memory orderings, prebuilt binary in CI

## Goal

Owner request (2026-10-27): a source-analysis gate that makes it impossible to silently weaken the memory orderings of `WriterDrainBarrier` (card RV-ENGINE-898ECB5E). A loom model (stage F14) proves the algorithm on a `#[cfg(loom)]` fence variant, but the production branch (`#[cfg(not(loom))]` or no cfg) is never compiled under loom. This gate pins that production branch.

The gate must be AST-based and live in the existing `tools/rust-guard` binary (syn 2, `full` + `visit`). CI must run a PREBUILT, cached rust-guard binary, not recompile it on every job.

## Part A — `rust-guard orderings` subcommand

1. New module `tools/rust-guard/src/orderings_cmd/`, with `mod.rs` holding re-exports only. Pick the files you need (for example `pins.rs` for the TOML model, `scan.rs` for the syn visitor, `run.rs`), keeping within 7 entries per folder and 1000 lines per file. Wire it into `main.rs` the same way as the existing `modules`/`structure` subcommands. Read those first and copy their CLI, finding and exit-code conventions (`common/finding.rs`).
2. Pin file: `.rust-guard/atomic_orderings.toml`.
   - Each `[[pin]]` has `path`, `item` (`fn_name` or `Type::fn_name`; for a `Drop` impl use `Type::drop`), `branch = "production"` (default) or `"loom"`, `owner` and `reason`.
   - It also has `calls`, a list of `{ method = "fetch_add" | "fetch_sub" | "load" | "store" | "swap" | "compare_exchange" | "compare_exchange_weak" | "fetch_update" | ..., min = "SeqCst" | "AcqRel" | "Acquire" | "Release" | "Relaxed" }`, plus an optional `require_fence = "SeqCst"`.
   - The `production` branch is the item without a `#[cfg(loom)]` attribute. That covers both `#[cfg(not(loom))]` and no cfg at all. If both a production and a loom definition exist, each is matched by its own pin.
3. Checks. Each finding carries `path:line`.
   - **Missing item:** a pinned item is not found.
   - **Weakened ordering:** an atomic method call on the pinned item has an ordering argument weaker than `min`. Strength order: Relaxed < Acquire = Release < AcqRel < SeqCst. For `compare_exchange*` and `fetch_update`, check both the success and the failure ordering against `min` (the failure ordering is capped at Acquire when `min` is `AcqRel` or `SeqCst`: Release/AcqRel are not valid failure orderings, so Acquire or SeqCst satisfies the pin).
   - **Non-literal ordering:** the argument is not a path ending in `Ordering::<Variant>` (a const, a variable or a macro). Banned in pinned items.
   - **Unpinned call:** an atomic method call on the pinned item that matches no `calls` entry.
   - **Missing fence:** `require_fence` is set, but no `fence(Ordering::SeqCst)` or `atomic::fence(...)` call is present.
   - **Stale pin:** a pinned call no longer exists.
   - A method is "atomic" by name, from the known set above, when it is called with an `Ordering` argument. That avoids having to resolve types.
4. Initial pins: every atomic call and fence of `WriterDrainBarrier` in `crates/shamir-engine/src/table/support/guards/writer_drain_barrier.rs` (`enter_writer`, the guard's `Drop`, `drain`, plus `begin_drain` / `is_drained` if they exist), all at their CURRENT master orderings. Production keeps `SeqCst` as it is today. Read the file and pin exactly what is there now. Stage F14 will later add a `#[cfg(loom)]` variant and must keep production unchanged; it may add `branch = "loom"` pins requiring `fence(SeqCst)`.
5. Self-tests go in the existing rust-guard test layout (follow how other subcommands' tests are organized and run in the `rust-guard self-test` CI job). Use fixture sources covering:
   - a weakened ordering
   - a removed fence
   - a renamed fn
   - a const-wrapped ordering
   - an extra unpinned call
   - production vs loom branch selection
   - the real pin file passing against the real source

   Each fixture must fail for exactly its intended reason.

## Part B — CI: run the gate and use a prebuilt, cached binary

1. In `.github/workflows/ci.yml`, every job that does `cargo build --manifest-path tools/rust-guard/Cargo.toml --locked` (lines around 687 and 805 today, and any others):
   - Put an `actions/cache` step (use the same action version already pinned elsewhere in the workflow) before it, caching `tools/rust-guard/target/debug/rust-guard` (plus `.exe` on Windows if any job runs there).
   - Key: `rust-guard-bin-${{ runner.os }}-${{ hashFiles('tools/rust-guard/**/*.rs', 'tools/rust-guard/Cargo.toml', 'tools/rust-guard/Cargo.lock', 'rust-toolchain.toml') }}`.
   - Make the build step conditional on a cache miss (`if: steps.<id>.outputs.cache-hit != 'true'`).
   - Keep the existing `Swatinem/rust-cache` for the miss path.
2. Add a step `rust-guard orderings` right after `rust-guard modules (whole repo)` in the source-integrity job: `tools/rust-guard/target/debug/rust-guard orderings --repo . --pins .rust-guard/atomic_orderings.toml`. It runs on draft and ready PRs and on master pushes, like the other source-integrity gates.
3. If the merge-gate job enumerates required jobs, no change is needed; the new step lives in an existing job. Verify this.

## Part C — docs

- `tools/rust-guard/README.md`: add an `orderings` section covering the pin format and the findings.
- `CLAUDE.md`, section "🛡️ rust-guard source-integrity gates (CI)": add one bullet for `orderings` (what it pins, where the pin file lives, and that changing a pinned ordering requires editing the pin with `owner` and `reason`). Also note in one line that CI caches the rust-guard binary by source hash.

## Requirements

- Allowed local commands: `cargo fmt --manifest-path tools/rust-guard/Cargo.toml -- --check`, `cargo clippy --manifest-path tools/rust-guard/Cargo.toml --all-targets --locked -- -D warnings`, `cargo check --manifest-path tools/rust-guard/Cargo.toml --all-targets`, and `python -I scripts/ci/check_layout.py`.
- Do NOT run the rust-guard binary or its tests locally. They run in CI.
- No new dependencies in rust-guard unless strictly needed. syn, toml and serde are already present. If `tools/rust-guard/Cargo.lock` must change, say so; the orchestrator regenerates it.
- Do not touch `crates/`, except reading `writer_drain_barrier.rs` to write the pins.
- Final message: files changed, the pin list, and each self-test with the reason it fails.

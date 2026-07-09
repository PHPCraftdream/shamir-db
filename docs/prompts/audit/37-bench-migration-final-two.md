Task: migrate the last two remaining criterion-based benches to this
repo's `bench-scale-tool::Harness` fixed-iteration convention (task
#493 — originally scoped as "41 benches" but earlier campaign tasks
already migrated the rest; only 2 remain).

## Scope

```
crates/shamir-connect/benches/hot_paths.rs
crates/shamir-engine/benches/backend_matrix.rs
```

Both still use `criterion` directly. Every other bench in this
workspace has already been migrated to the `bench-scale-tool::Harness`
pattern — use one of the already-migrated benches as your reference
template (e.g. `crates/shamir-index/benches/create_index_streaming.rs`,
`crates/shamir-index/benches/posting_cache_hit.rs`,
`crates/shamir-storage/benches/storage_fjall_pump.rs`,
`crates/shamir-storage/benches/storage_cached_pump.rs`,
`crates/shamir-funclib/benches/distinct_arrays.rs`,
`crates/shamir-transport-tcp/benches/framing.rs` — read 2-3 of these
first to internalize the convention before touching anything).

## What "migrate" means here

1. Replace the `criterion::Criterion`/`criterion_group!`/`criterion_main!`
   scaffolding with the `bench-scale-tool::Harness` fixed-iteration
   pattern used by the reference benches above.
2. Every bench function must call
   `shamir_bench_utils::tune(&mut group, sample_size, measurement_secs,
   warm_up_secs)` (or the individual `sample_size`/`measurement_time`/
   `warm_up_time` helpers) so QUICK mode (the default —
   `BENCH_QUICK=1`-equivalent fast iteration) actually kicks in, per
   this repo's CLAUDE.md bench-methodology section. Do NOT leave any
   bench function on raw Criterion defaults (that hits
   minutes-per-variant, which is exactly what this migration exists to
   avoid).
3. Preserve what each bench actually MEASURES (the same code paths,
   same inputs/parameterization) — this is a harness-only swap, not a
   redesign of what's being benchmarked. If a bench's current
   parameterization is unclear or seems stale (e.g. references removed
   APIs), investigate and adapt it to the current API surface rather
   than deleting coverage, but do not invent NEW benchmark scenarios
   beyond what's already there.
4. Update `Cargo.toml` bench registration (`[[bench]]` sections) for
   both `shamir-connect` and `shamir-engine` crates as needed — check
   the reference benches' `Cargo.toml` entries for the exact pattern
   (dev-dependency on `bench-scale-tool`/`shamir-bench-utils` or
   equivalent internal crate name — grep the reference benches'
   Cargo.toml to find the actual dependency name used in this repo).
5. Verify each migrated bench still COMPILES and RUNS correctly with
   `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p
   <crate> --bench <name>` (per CLAUDE.md's bench cache isolation
   rule) — confirm output looks sane (not necessarily record exact
   numbers; this is a harness swap, not a perf optimization, so there's
   no before/after speedup to report — just confirm the bench compiles,
   runs, and produces plausible-looking measurements for each scenario
   it covers).

## What NOT to do

- Do not change what's being measured (no new perf investigation, no
  code changes to the benchmarked subsystems themselves — this task is
  purely about the bench HARNESS).
- Do not touch any other bench file — only these two.
- Do not delete bench coverage; if a scenario in the current criterion
  bench references dead/removed code, adapt the migration to the
  current equivalent rather than silently dropping it — note any such
  adaptation in your report.

## Gate

```
cargo fmt -p shamir-connect -p shamir-engine -- --check
cargo clippy -p shamir-connect -p shamir-engine --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[hot_paths.rs] Status: migrated
  > What it measures (unchanged), confirmation it compiles + runs
[backend_matrix.rs] Status: migrated
  > What it measures (unchanged), confirmation it compiles + runs
```
Full gate results (exact commands + pass/fail). Note any adaptation
needed for stale/removed API references.

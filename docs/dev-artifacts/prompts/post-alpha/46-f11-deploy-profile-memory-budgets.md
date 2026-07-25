# Brief for #801 (F-11) — finite `max_inflight_response_bytes` in shipped deploy profiles

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The gap

`QueryLimitsConfig` (`crates/shamir-server/src/config.rs:341-385`) has:

```rust
pub struct QueryLimitsConfig {
    pub max_result_size_bytes: usize,       // default 64 MiB
    pub max_execution_time_secs: u64,       // default 60
    pub max_queries_per_batch: usize,       // default 100
    pub max_inflight_response_bytes: Option<usize>,  // default None (line 372)
}
```

`max_inflight_response_bytes` (RI-15, task #754) is the global pessimistic
upfront-reservation budget that bounds total in-flight response bytes
across all concurrently-executing batches — per the CR-B2 upfront
reservation model documented in `docs/guide-docs/guide/07-operations.md:197-220`,
it acts as a hard concurrency limiter:
`effective_concurrency ≈ max_inflight_response_bytes / max_result_size_bytes`.
When it's `None` (unbounded), that concurrency ceiling simply doesn't
exist — an operator who deploys a memory-constrained container gets no
protection from this specific budget, regardless of how carefully every
other limit in the same profile was sized.

Three shipped example configs exist under `deploy/`:

- `deploy/server.example.ktav` — the "reference/all-fields-shown" example.
  Its `security.query_limits` block (~line 159-164) **already sets**
  `max_inflight_response_bytes: 4294967296` (4 GiB) alongside
  `max_result_size_bytes: 1 GiB` (a 4x ratio) and
  `max_active_connections: 10000`.
- `deploy/server.small.example.ktav` — sized for a 1-2 GiB container.
  `query_limits` block (~line 84-91) has **no `max_inflight_response_bytes`
  key at all** → deserializes to `None` (unbounded). Has
  `max_result_size_bytes: 32 MiB`, `max_active_connections: 500`.
- `deploy/server.medium.example.ktav` — sized for a 4-8 GiB container.
  Same gap — `query_limits` block (~line 84-91) has no
  `max_inflight_response_bytes` key. Has `max_result_size_bytes: 64 MiB`,
  `max_active_connections: 2000`.

There is no "large" profile shipped today — only small/medium/reference.
Do not invent one; size only the two that exist.

## The fix — set a finite value in both `small` and `medium`

Add `max_inflight_response_bytes` to the `security.query_limits` block of
**both** `deploy/server.small.example.ktav` and
`deploy/server.medium.example.ktav` (do NOT touch
`deploy/server.example.ktav` — it already has a value and is the
reference/all-fields example, not a sizing target).

Derive the value from the SAME ratio the reference profile already uses
(`max_inflight_response_bytes / max_result_size_bytes = 4`), applied to
each profile's own `max_result_size_bytes`:

- `small`: `max_result_size_bytes = 32 MiB` → `max_inflight_response_bytes
  = 128 MiB` (134217728). Sanity-check against the small profile's
  documented ~1-2 GiB container target and its own sizing-formula header
  comment (lines 1-11 of the file) — 128 MiB inflight budget must leave
  ample headroom for the Argon2 RAM allocation and the rest of the process
  footprint already accounted for in that header. If the arithmetic
  doesn't leave comfortable headroom, adjust down and explain why in the
  file's existing header-comment style (do not silently pick a number).
- `medium`: `max_result_size_bytes = 64 MiB` → `max_inflight_response_bytes
  = 256 MiB` (268435456). Same headroom sanity-check against the medium
  profile's ~4-8 GiB target.

Add a one-line comment next to the new key (matching this file's existing
inline-comment style — check how neighboring `query_limits` keys are
already commented) explaining it's sized at 4x `max_result_size_bytes`,
matching the reference profile's ratio, and citing RI-15 /
`docs/guide-docs/guide/07-operations.md`'s concurrency-math paragraph for
why this ratio determines effective concurrency
(`max_inflight_response_bytes / max_result_size_bytes` concurrent
batches).

## Tests

`crates/shamir-server/src/tests/config_tests.rs` (wired via `pub mod
config_tests;` in `tests/mod.rs`) already has `small_profile_parses_and_validates`
and `medium_profile_parses_and_validates` (asserting
`max_active_connections`/`max_result_size_bytes` against a `deploy_path()`
helper that resolves example files via `CARGO_MANIFEST_DIR`) plus an RI-15
section (`default_max_inflight_response_bytes_is_none`,
`inflight_budget_below_result_cap_is_rejected`,
`inflight_budget_at_or_above_result_cap_is_accepted`).

Extend the two existing `*_profile_parses_and_validates` tests (or add a
sibling assertion in each, matching the file's existing style) to assert:

```rust
assert_eq!(
    cfg.security.query_limits.max_inflight_response_bytes,
    Some(134_217_728), // small: 128 MiB
);
```

and the medium equivalent (`Some(268_435_456)`, 256 MiB) — using whichever
literal form (raw integer vs. a `_MiB` const/helper, if one already exists
in this test file) matches this file's existing convention. This is a pure
regression guard: every shipped profile must have `Some(<finite value>)`,
never `None`, going forward.

## Constraints

- Do NOT touch `deploy/server.example.ktav` (already correctly set).
- Do NOT invent a "large" profile — only small/medium exist.
- Do NOT change `max_result_size_bytes`, `max_active_connections`, or any
  other existing field in either profile — this task ONLY adds the
  missing `max_inflight_response_bytes` key.
- Do NOT change `QueryLimitsConfig`'s Rust-level default (`None` stays the
  correct default for users who don't start from one of the shipped
  example profiles — this task only closes the gap in the SHIPPED
  EXAMPLES, not the struct's own default).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-server` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must stay
  clean (this task is mostly `.ktav`/doc edits plus a couple of test
  assertions, so this should be a no-op check).
- Follow workspace conventions: surgical diff, no incidental reformatting
  of the `.ktav` files beyond the one new key per profile.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- config
```

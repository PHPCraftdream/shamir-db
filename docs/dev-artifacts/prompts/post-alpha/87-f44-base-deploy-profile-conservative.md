# Brief for F-44 (#852, P1) — make the base deploy profile conservative; move high-capacity numbers to a new `large` profile

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P1-5) found `deploy/server.example.ktav` (the FIRST config a new
operator copies) sets:

- `max_active_connections: 10000`
- `max_result_size_bytes: 1073741824` (1 GiB)
- `max_inflight_response_bytes: 4294967296` (4 GiB)

— all far above the CODE's own safer defaults (confirm yourself in
`crates/shamir-server/src/config.rs`: `default_max_active_connections()` =
1000, `default_max_result_size_bytes()` = 64 MiB, and
`default_max_inflight_response_bytes()` = `4 × 64 MiB` = 256 MiB, per F-29
#822). For a small/indie-product base example, shipping numbers this far
above the library's own defaults effectively cancels the safe-by-default
posture and invites gigabyte-scale responses out of the box, for a new
operator who copied the FIRST file they saw with no reason to think it was
already tuned for high capacity.

**Confirmed no existing test asserts on the base file's specific resource
numbers** — check `crates/shamir-server/src/tests/config_tests.rs`'s
`reference_example_parses_and_validates` yourself (it only checks
parse+`validate()`, not specific values), so changing these numbers is
low-risk. Also confirmed the base file (unlike medium/small) doesn't even
set `max_active_connections_per_ip` at all — check yourself.

## What to build

### 1. Make `deploy/server.example.ktav`'s `security` block conservative

Change its `connection`/`query_limits` values to match the CODE's own
defaults exactly (not the medium profile's — the base example should be
"what you get if you barely configure anything", matching the library
baseline precisely):

- `max_active_connections: 1000` (was 10000)
- add `max_active_connections_per_ip: 100` (currently absent from this
  file — present in medium/small, add it here too for consistency)
- `max_result_size_bytes: 67108864` (64 MiB, was 1 GiB)
- `max_inflight_response_bytes: 268435456` (256 MiB, was 4 GiB)
- Update the surrounding comments (currently say "1 GiB"/"4 GiB" inline)
  to match the new values — check `server.medium.example.ktav`'s comment
  style for the exact wording convention to mirror.
- `max_execution_time_secs`/`max_queries_per_batch` are unchanged (60/100
  — already match code defaults, confirm yourself).

### 2. New `deploy/server.large.example.ktav` — the OLD numbers, explicitly named

Create a new profile carrying the base file's PRE-fix numbers (10000
connections, 1 GiB result, 4 GiB inflight) so an operator who genuinely
wants/needs high capacity has an explicitly-named place to find them,
instead of them being the accidental default. Copy the base file's FULL
structure (same `data_dir`/`logging`/`kdf_defaults`/`argon2_concurrent_max`/
`listeners`/`tls`/`audit`/`observability` sections — check whether
medium/small profiles duplicate ALL sections or only override
`security`, and mirror whichever pattern this codebase already
establishes) with only the `security.connection`/`security.query_limits`
values set to the high-capacity numbers, and a clear top-of-file comment
explaining this is the explicitly-opt-in high-capacity profile (mirroring
`server.medium.example.ktav`'s/`server.small.example.ktav`'s own
introductory comment style).

## Tests — MANDATORY, in the same commit

Add a `large_profile_parses_and_validates` test to
`crates/shamir-server/src/tests/config_tests.rs`, mirroring
`medium_profile_parses_and_validates`'s/`small_profile_...`'s existing
shape exactly (parse + `validate()` + assert the specific
`max_active_connections`/`max_result_size_bytes`/
`max_inflight_response_bytes` values match what you put in the new file).
Also add the new `"server.large.example.ktav"` filename to
`shipped_profiles_leave_experimental_migration_api_disabled`'s existing
list (~line 425-429) — every shipped profile must stay pinned there.

## Constraints

- Do NOT touch `crates/shamir-server/src/config.rs`'s actual code
  defaults — this task only changes the shipped example `.ktav` files.
- Do NOT touch `server.medium.example.ktav`/`server.small.example.ktav`.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-server -- --check` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean (docs-only changes to `.ktav` files won't affect these, but the
  new test code must still pass both).

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- profile
./scripts/test.sh -p shamir-server --full
```

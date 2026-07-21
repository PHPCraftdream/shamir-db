# RI-8: Safe resource profiles — Argon2, result cap, connection limits

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context (investigated already — do not re-derive, just implement)

Review 2026-07-20, P0#6: the shipped defaults are dangerous for the
project's stated audience ("small/medium projects"):

- `deploy/server.example.ktav`: `kdf_defaults.memory_kb = 131072` (128 MiB)
  × `argon2_concurrent_max = 64` → up to 8 GiB peak auth-RAM, against a
  typical systemd/Docker memory limit of 4 GiB.
- `crates/shamir-server/src/config.rs:310` (`default_max_result_size_bytes`)
  — code-level default `1024*1024*1024` (1 GiB) — a single batch response
  can be clamped to 1 GiB by default.
- `crates/shamir-server/src/config.rs:360` (`default_max_active_connections`)
  — code-level default `10_000`.

There is currently only ONE example config file:
`deploy/server.example.ktav`. There is currently NO `config` test file
under `crates/shamir-server/src/tests/` (checked: no `config_tests.rs`
exists; `crates/shamir-server/src/config.rs` itself has zero `#[cfg(test)]`
code). `crates/shamir-server/src/tests/mod.rs` is the manifest file wiring
in sibling test modules — follow its existing pattern.

No RAM-detection facility exists anywhere in the workspace (`shamir-numa`
and `shamir-tunables` were checked — neither reads available/total RAM, no
`sysinfo`-style dependency exists anywhere in any `Cargo.toml`). Adding one
would mean a new cross-platform dependency purely for this (and container
cgroup v1/v2 memory-limit detection vs. host RAM is genuinely fiddly) — **do
NOT add a RAM-detection dependency.** Document the sizing formula instead
(see below) — this is the DECIDED choice for item 4 below, already made,
do not re-litigate.

## The task — DECIDED numbers, implement exactly these

### 1. Two new example config profiles in `deploy/`

Create `deploy/server.small.example.ktav` and
`deploy/server.medium.example.ktav`. Each is a FULL, valid, standalone ktav
config — copy the complete structure of the existing
`deploy/server.example.ktav` (all sections: `data_dir`, `logging`,
`kdf_defaults`, `argon2_concurrent_max`, `listeners`, `tls`, `security`,
`audit`, `observability`) and adjust only the fields below. Keep every
other field/comment from the existing file as-is (paths, listener
addresses, TLS block, audit block, observability block) so both new files
stay drop-in-comparable to the original.

**`server.small.example.ktav`** — target deployment: 1–2 GiB container RAM
(sized for ~1.5 GiB):
```
kdf_defaults: {
    memory_kb: 65536     # 64 MiB
    time: 4
    parallelism: 1
    argon2_version: 19
}
argon2_concurrent_max: 6
# 6 × 64 MiB = 384 MiB worst-case auth RAM (~25% of a 1.5 GiB budget)
```
```
security: {
    connection: {
        auth_init_timeout_ms: 5000
        max_active_connections: 500
        max_active_connections_per_ip: 25
    }
    query_limits: {
        max_result_size_bytes:    33554432   # 32 MiB
        max_execution_time_secs:  60
        max_queries_per_batch:    100
    }
}
```

**`server.medium.example.ktav`** — target deployment: 4–8 GiB container RAM
(sized for ~6 GiB):
```
kdf_defaults: {
    memory_kb: 131072    # 128 MiB (spec §3.7.2 default)
    time: 4
    parallelism: 1
    argon2_version: 19
}
argon2_concurrent_max: 12
# 12 × 128 MiB = 1536 MiB worst-case auth RAM (~25% of a 6 GiB budget)
```
```
security: {
    connection: {
        auth_init_timeout_ms: 5000
        max_active_connections: 2000
        max_active_connections_per_ip: 100
    }
    query_limits: {
        max_result_size_bytes:    67108864   # 64 MiB
        max_execution_time_secs:  60
        max_queries_per_batch:    100
    }
}
```

Add a header comment block (above `data_dir:`) to BOTH new files stating:
the target RAM budget, and the general sizing formula so an operator can
derive their own numbers for a different budget:

```
# Sizing formula for this profile's Argon2 auth-RAM ceiling:
#   argon2_concurrent_max × kdf_defaults.memory_kb (KiB)  ≤  ~25% of your
#   container/host RAM (KiB).
# Example: a 4 GiB container → ~1 GiB budget → memory_kb=131072 (128 MiB)
# allows argon2_concurrent_max up to ~8.
```

### 2. Lower the code-level default result-size cap

`crates/shamir-server/src/config.rs::default_max_result_size_bytes()`:
change `1024 * 1024 * 1024` (1 GiB) → `64 * 1024 * 1024` (64 MiB). Update
the doc comment on `QueryLimitsConfig::max_result_size_bytes` (currently
says "Default 1 GiB") to say "Default 64 MiB". This is a BEHAVIORAL
default change (operators relying on the implicit 1 GiB default without an
explicit `security.query_limits` block will now get a 64 MiB clamp) — an
operator who genuinely needs more sets `max_result_size_bytes` explicitly
in their config, which already overrides the default.

### 3. Lower the code-level default connection cap

`crates/shamir-server/src/config.rs::default_max_active_connections()`:
change `10_000` → `1_000`. Update the doc comment on
`ConnectionSecurity::max_active_connections` (currently says "Default
10000") to say "Default 1000". Do NOT touch
`default_max_active_connections_per_ip()` (stays `100` — already a
reasonable 10% of the new global cap, no change needed, and the task does
not ask for it).

### 4. RAM-based Argon2 auto-derivation — explicitly SKIPPED (documented, not coded)

Already decided above — the sizing formula lives as a comment in both new
example files (item 1). Do NOT add any RAM-detection code or dependency.

### 5. Global inflight response-memory budget — assess, do not implement (unless trivial)

Read `crates/shamir-server/src/connection/request_loop.rs` (and whatever
enforces `max_result_size_bytes` today per-batch) to confirm: is there
currently any mechanism that bounds the SUM of in-flight response bytes
across all concurrently-executing batches/connections (as opposed to each
batch individually being clamped to `max_result_size_bytes`)? The prior
investigation for this brief did not find one. If implementing a global
byte-budget semaphore across concurrent requests is straightforward (e.g.
a single `tokio::sync::Semaphore`-style byte-budget gate sitting where
`max_result_size_bytes` is already enforced, with no larger architectural
change), implement it as a `security.query_limits` field (e.g.
`max_inflight_response_bytes`, default unset = unbounded, matching the
"omit sections is behavior-preserving" pattern already used elsewhere in
this file). If it turns out to need deeper changes (new shared state
threaded through the connection/request dispatch pipeline, cross-cutting
lifecycle management, etc.), STOP — do not implement it — and instead
report in your summary exactly why it's nontrivial, precisely enough that
a dedicated follow-up task can be scoped from your notes.

## Tests (MANDATORY — behavioral default changes require test coverage)

Create `crates/shamir-server/src/tests/config_tests.rs` (new file — none
exists yet) and wire it into `crates/shamir-server/src/tests/mod.rs`
(`pub mod config_tests;`, alongside the other `pub mod ..._tests;` lines,
matching the existing manifest-only pattern in that file). Cover:

1. `QueryLimitsConfig::default().max_result_size_bytes == 64 * 1024 * 1024`.
2. `ConnectionSecurity::default().max_active_connections == 1_000` (and
   `max_active_connections_per_ip` still `100`, proving it was
   deliberately left unchanged).
3. Both new example files parse successfully:
   `Config::from_file(Path::new("../../deploy/server.small.example.ktav"))`
   and the `medium` twin (adjust the relative path to whatever actually
   resolves from `crates/shamir-server`'s test working directory — verify
   empirically, e.g. by checking how `deploy/server.example.ktav` itself
   might already be exercised by an existing test, or resolve via
   `CARGO_MANIFEST_DIR`), AND `Config::validate()` returns `Ok(())` for
   both.
4. For each of the two new profiles, assert the Argon2 ceiling exactly:
   `small: 6_u64 * 65_536 == 393_216` (KiB) and
   `medium: 12_u64 * 131_072 == 1_572_864` (KiB) — parsed directly from the
   loaded `Config` (`argon2_concurrent_max as u64 * kdf_defaults.memory_kb
   as u64`), not hand-recomputed constants, so the test actually fails if
   the shipped file's numbers ever drift from what this brief specifies.

## Docs

- `deploy/README.md`: add a short "## Resource profiles" section (place it
  near the existing "## Quick start" sections) pointing at
  `server.small.example.ktav` / `server.medium.example.ktav` /
  `server.example.ktav`, one line each on target RAM and when to pick
  which, plus the sizing formula from item 1.
- `CHANGELOG.md`: under `## [Unreleased]`, add a bullet documenting the two
  code-level default changes (result-size cap 1 GiB → 64 MiB,
  max_active_connections 10000 → 1000) — follow the existing bullet style
  in that section (see the RI-3 entry already there for tone/format).

## Out of scope

- Do NOT touch `default_argon2_max()` (stays `64` — the spec §8 default;
  it is not a per-profile constant, both new example files set their own
  explicit `argon2_concurrent_max` which overrides it).
- Do NOT touch `kdf_defaults` validation floors (`KDF_MIN_MEMORY_KB` etc.)
  — those are spec §3.7.2 normative floors, unrelated to this task.
- Do NOT modify `deploy/server.example.ktav` itself — it stays as the
  existing "reference/all-fields-shown" example; the two new files are
  additions, not replacements.
- Do NOT add a RAM-detection dependency (see item 4).

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-server --full` green, including your new
  `config_tests.rs`.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report literal command output for all of the above, plus your finding
  from item 5 (implemented, or a precise nontrivial-reason writeup).

# Brief for F-21 (#814, P2) — add `max_inflight_response_bytes` to the base `deploy/server.example.ktav`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

`security.query_limits.max_inflight_response_bytes` (RI-15's global
upfront-reserve budget on total in-flight response bytes,
`crates/shamir-server/src/config.rs` ~line 371-376) already appears,
documented, in `deploy/server.medium.example.ktav` and
`deploy/server.small.example.ktav` — but is MISSING from the base
`deploy/server.example.ktav`, so an operator starting from the base
reference config alone never sees this knob at all (it silently defaults
to `None` = unbounded — a separate, broader task, F-29/#822, is tracking
whether that default itself should change; this task is just the doc/config
gap in the base example file).

## What to add

In `deploy/server.example.ktav`, inside the existing `security.query_limits`
block (currently ends after `max_queries_per_batch: 100` at ~line 80-81),
add a `max_inflight_response_bytes` entry, sized the SAME way the medium/
small profiles already size theirs — 4× that file's own
`max_result_size_bytes` (the base file's `max_result_size_bytes` is
`1073741824`, 1 GiB, so 4× = `4294967296`, 4 GiB) — and mirror the
medium/small files' comment wording (RI-15 budget, sized 4× for headroom,
pointer to `docs/guide-docs/guide/07-operations.md`). Match this file's
own existing comment style/formatting (short comment above the field,
`# N MiB/GiB.` convention already used elsewhere in this same file, e.g.
`max_result_size_bytes`'s own `# 1 GiB.` comment).

## Constraints

- Do NOT touch `deploy/server.medium.example.ktav` or
  `deploy/server.small.example.ktav` — already correct.
- Do NOT change any other field in `deploy/server.example.ktav`.
- Do NOT touch `crates/shamir-server/src/config.rs` — this task is the
  example-file gap only, not a default-value change (that's F-29/#822).
- This is a plain-text config file, not Rust — no `fmt`/`clippy` applies.
  If a test asserts the shape of `deploy/server.example.ktav` (check
  `crates/shamir-server/src/tests/config_tests.rs` for anything that
  parses the example files), confirm it still passes after the addition.

## Verification the orchestrator will run

```
./scripts/test.sh -p shamir-server -- config
```

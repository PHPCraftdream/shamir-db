# Brief for F-29 (#822, P1) — finite safe default for `max_inflight_response_bytes` + startup warning + metrics

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

`security.query_limits.max_inflight_response_bytes` (RI-15's global
in-flight response-byte budget, `crates/shamir-server/src/config.rs`
~line 371-376, 385) defaults to `None` — meaning, per
`crates/shamir-server/src/byte_budget.rs` (~line 55-58, 86-98), the
server-wide budget is **unbounded**: only the per-batch
`max_result_size_bytes` cap applies, and there is no ceiling on the SUM of
simultaneously in-flight response bytes across every connection. The
module's own doc comment states the risk precisely: at
`max_active_connections = 1000` × a 64 MiB per-batch cap, worst case is
~64 GiB of buffered response memory — unbounded relative to a typical
4-8 GiB container.

The shipped `deploy/server.medium.example.ktav` and
`deploy/server.small.example.ktav` profiles already set a finite value
(4× their own `max_result_size_bytes`) — only the BASE
`deploy/server.example.ktav` was missing it (fixed separately, F-21/#814,
already landed) and only the CODE DEFAULT (`Config`'s `Default` impl, not
any example file) remains `None`.

This task is genuinely a design decision with a real backward-
compatibility tradeoff — investigate and decide, don't just mechanically
pick a number:

## Investigate and decide

1. **Is changing the code default to a finite value safe?** A server
   currently running with NO `max_inflight_response_bytes` configured (the
   common case for anyone who hasn't read RI-15's docs) currently gets
   unbounded behavior. Silently switching the default to a finite cap
   changes behavior for every such deployment on upgrade — a legitimate,
   previously-working high-concurrency workload could start hitting the
   new cap and blocking/backpressuring where it never did before.
   Investigate what `ByteBudget::acquire`'s behavior actually is when the
   budget is exhausted (does it block/wait, or reject outright? check
   `byte_budget.rs`'s `acquire` method and its callers in
   `db_handler/handler.rs`) — this determines whether "hits the new
   default cap" means "gets slower" (a bounded wait) or "gets a hard
   error" (a behavior break), which materially changes how risky a
   default change is.
2. **Pick the actual default value.** If you conclude a finite default is
   safe/desirable, the natural choice (matching the shipped profiles' own
   established convention) is `4 × max_result_size_bytes`'s DEFAULT value
   (`default_max_result_size_bytes()`, currently 64 MiB → 256 MiB), NOT a
   hardcoded constant independent of that default — so if
   `default_max_result_size_bytes()` ever changes, the derived
   `max_inflight_response_bytes` default tracks it automatically. Check
   `Config::validate`'s existing invariant (`max_inflight >= max_result`,
   ~line 649-657) — a derived-from-the-same-source default trivially
   satisfies this by construction.
3. **Startup warning — for which case exactly?** Given the above, decide
   precisely what triggers the warning: is it "operator explicitly set
   `max_inflight_response_bytes` to some HUGE value (or the config schema
   still allows explicit `null`/`None` to mean unbounded, if you decide to
   keep that escape hatch) and we want to flag the operator is opting into
   unbounded behavior"? Or is it simply "log the computed default at boot
   so an operator relying on the default knows what ceiling they're
   getting" (an informational log line, not a warning about risk)? Pick
   the interpretation that matches "startup warning on None" from the
   task's own framing — investigate whether `Option<usize>` should REMAIN
   the config type (so an operator CAN still explicitly opt into
   `None`/unbounded, e.g., by setting `max_inflight_response_bytes: null`
   in their `.ktav`) with a `tracing::warn!`/`log::warn!` at boot when
   that explicit choice is detected, while the DEFAULT (absence of the
   key entirely) resolves to the finite value from step 2. This preserves
   an intentional escape hatch while making the DEFAULT safe — check
   whether the `.ktav` config format can distinguish "key absent" from
   "key explicitly null" (if it can't, and `serde(default)` collapses both
   to the same `None`, state this clearly and pick the simpler design:
   either the escape hatch is removed entirely, or the warning fires
   whenever the resolved value is `None` regardless of how it got there —
   your call, but be explicit about which you chose and why).
4. **Reserved-bytes metrics.** `ByteBudget` already exposes `used()` and
   `cap()` (`byte_budget.rs` ~line 211-216) — no new instrumentation
   needed on that side. Add gauges following this codebase's EXISTING
   metrics convention exactly (`crates/shamir-server/src/observability.rs`
   ~line 155-290 — `metrics::describe_gauge!` + zero-touch
   `metrics::gauge!(...).set(0.0)` registration at spawn, then a
   snapshot inside the existing background poller loop, alongside the
   already-present `shamir_tx_*`/`shamir_gc_*` gauges). Suggested names
   (adjust to match this file's existing naming convention exactly, check
   for a `shamir_` prefix + `_bytes`/`_total` suffix pattern):
   `shamir_inflight_response_bytes_used` (current `used()`) and
   `shamir_inflight_response_bytes_cap` (current `cap()`, or a sentinel
   like `-1`/`f64::INFINITY` if truly unbounded — check how this file's
   existing gauges represent "not applicable" values, if any precedent
   exists, otherwise use your judgment and document the choice).

## Tests

**MANDATORY, test-then-fix in the same commit**:

1. Confirm the new default: a `Config` built via `Default`/deserializing
   an empty/minimal `.ktav` with no `query_limits.max_inflight_response_bytes`
   key resolves to the finite computed default (256 MiB or whatever you
   land on), not `None`.
2. Confirm `Config::validate`'s existing invariant (`max_inflight >=
   max_result`) still holds and is still tested for the EXPLICIT-override
   case (an operator who sets both) — this shouldn't change, just confirm
   no regression.
3. If you kept an explicit-`None`/unbounded escape hatch: a test
   confirming that path still resolves to a genuinely unbounded
   `ByteBudget` (existing `acquire`-never-blocks behavior preserved) and
   that the startup warning path is reachable (check this crate's
   existing convention for asserting a log line was emitted, if one
   exists — e.g. a test log-capture harness; if none exists, it's
   acceptable to test only the resolved `Option<usize>` value and
   note in your summary that the warning ITSELF isn't directly asserted).
4. A test confirming the new metrics gauges appear correctly (check this
   crate's existing test convention for asserting `/metrics` output, e.g.
   scraping the observability HTTP endpoint in a test, matching how the
   existing `shamir_tx_*` gauges are presumably already tested).

## Constraints

- Do NOT touch `deploy/server.example.ktav` or the medium/small profiles
  — F-21/#814 already handled those; this task is about the CODE default
  and startup/metrics behavior only.
- Do NOT touch `ByteBudget`'s core acquire/release logic — only its
  construction-time default and (if you add gauges) read-only snapshot
  calls (`used()`/`cap()`), which already exist.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-server -- --check` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.

## Docs

Update `docs/guide-docs/KNOWN_LIMITATIONS.md` if it currently documents
`max_inflight_response_bytes`'s default as unbounded (search for it) —
correct it to reflect the new finite default and, if you kept an escape
hatch, document how to opt back into unbounded behavior explicitly.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- config
./scripts/test.sh -p shamir-server -- byte_budget
./scripts/test.sh -p shamir-server --full
```

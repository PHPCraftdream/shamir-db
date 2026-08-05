# Brief — #984: expose degraded (non-Ready) index state via /metrics

Task: #984 in the session TaskList. Follow-up from #966 (P1-1), which
deliberately left this open. Read this brief in full — a design constraint has
already been decided (below) and must not be re-litigated.

## What exists today, and what's missing

#966 made `doctor::verify()`'s `IndexHealth` report a `Building`-stuck
regular/unique/sorted index as unhealthy (state field + `is_healthy()` +
diagnostic message). That is **pull-based**: an operator or script must call
`TableManager::verify()` explicitly, and it does a **FULL data-store stream**
to compare expected-vs-actual entry counts.

Missing: a **push/passive** signal so a stuck-`Building` index is visible
without anyone invoking `doctor::verify()`. The original review (P1-1) asked
for "server readiness/metrics должны сигнализировать degraded index".

## Hard constraint — DECIDED, do not re-open

`crates/shamir-server/src/observability.rs`'s own module doc states `/readyz`
must stay cheap and must not depend on other subsystems (today it only checks
"listeners bound"). **Do NOT wire a `doctor::verify()`-style scan into
`/readyz`.** That would violate the documented invariant and make `/readyz`
flaky/slow under load — exactly what it is designed to avoid.

The signal you build must be **O(number of indexes)**, in-memory only, with
**zero data-store reads**. It reads the ALREADY-IN-MEMORY registries:
`IndexDefinition.state` / `SortedIndexDefinition.state` / the index2
descriptors.

## Required design

### 1. A cheap degraded-index count

Add a method that walks the in-memory index registries of a `TableManager` and
returns how many indexes are NOT in `Ready` state (plus, ideally, a small
breakdown — decide whether a per-family or per-state breakdown is worth it, and
justify). No `Store` access. No `scc::*::len()` — this repo bans it as O(N)
(`clippy.toml` `disallowed-methods`); if you need a count, iterate explicitly
or keep an `AtomicUsize` mirror.

### 2. Enumerating open tables cheaply — start here

`crates/shamir-engine/src/repo/repo_instance.rs` line 26:
`tables: Arc<TDashMap<String, Arc<OnceCell<TableManager>>>>` — already-open
tables are enumerable in memory. Find the equivalent level above it
(repo→db→server registry) before inventing anything new; report what you found.

**Only count ALREADY-OPEN tables** — do NOT force-open closed tables to inspect
them (that would turn a cheap gauge into an expensive scan and change server
behaviour as a side effect of scraping `/metrics`). Document this limitation
explicitly in the metric's `describe_gauge!` help text: the gauge reflects
currently-open tables only.

### 3. Prometheus gauge on the existing /metrics endpoint

`observability.rs` already has the Prometheus recorder wired and an established
pattern for exactly this shape: `tx_metrics: Option<Arc<shamir_tx::TxMetrics>>`
is passed into `spawn`/`spawn_with_byte_budget`, described up-front with
`metrics::describe_gauge!` (so the series appears in `/metrics` even before
the first event), and snapshotted by the existing **background poller every
5 s**. Mirror that pattern exactly — same optional-`Arc` plumbing, same
describe-at-registration, same poller cycle. Do NOT add a second poller task.

Name the gauge in the existing style (the file already has
`shamir_inflight_response_bytes_used`/`_cap`); `shamir_degraded_indexes_total`
is the suggested name from the task — keep it unless a sibling naming
convention argues otherwise, and say which you chose.

### 4. The /readyz question — answer it explicitly

Decide and justify ONE of:
- keep `/readyz` strictly binary and unchanged, relying on the new gauge alone;
- or reflect degradation in the richer `/healthz` / `/info` endpoint (which is
  already the "pretty-printed operator debugging" surface per the module doc).

`/readyz` must not become expensive either way. State your choice and reasoning
in the code comment, not just the report.

## Required tests

- A unit test that a table with a `Building`-stuck index reports a non-zero
  degraded count, and a fully-`Ready` table reports zero. Cover regular,
  unique, sorted, and index2 families (or state clearly which family cannot be
  put into a non-Ready state and why).
- A test asserting the count path performs **no store reads** — if there is an
  existing fault-injecting / counting `Store` test double in this workspace,
  use it (search `crates/shamir-engine/src/table/tests/` and
  `crates/shamir-storage/` for one); if none exists, say so and cover the
  intent as best you can rather than building new infrastructure.
- A metrics test in `shamir-server` asserting the gauge is present in
  `/metrics` output even at zero, following the existing
  `metrics_exposes_*` tests referenced in `observability.rs`'s doc comment
  (find them and copy their shape).

## Scope discipline

- Do NOT touch `doctor::verify()` / `IndexHealth` — this task is additive and
  independent of the pull-based audit.
- Do NOT add any data-store read to `/readyz`, `/healthz`, `/metrics`, or the
  poller.
- Do NOT open tables that aren't already open.
- Do NOT add a new background task or a new HTTP endpoint.

## Gate (MANDATORY)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-server --full
```

⚠️ Raw `cargo test` is BLOCKED by this repo's perimeter guard. Use
`./scripts/test.sh` (`-p <crate>`, `-- <substring>` for a narrow run).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the test
run, then commits. Only edit/create files and run read-only / test / gate
commands.

## What to report back

- The enumeration path you found (repo→db→server) and its exact cost.
- The gauge name + its `describe_gauge!` help text.
- Your `/readyz` decision and the reasoning.
- Proof the count path does zero store I/O (how you established it).
- The list of tests added.
- Exact gate command output.

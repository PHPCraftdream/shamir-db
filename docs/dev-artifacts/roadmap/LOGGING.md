בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Milestone: Logging — non-blocking, batched, namespaced

A logging system that never blocks the caller, writes in batches per config,
allocates nothing when a level/namespace is disabled, and is filterable by
namespace masks with per-namespace levels (changeable at runtime).

Best done WITH `GRACEFUL_SHUTDOWN.md` (shared writer thread + join on stop)
and `RUNTIME_MODES.md` (sink per mode).

## Two truths about "logs are scattered"
1. **Scattered is correct and decoupled — not bloat.** An event is logged
   where it happens; the data is local to that site. The `log` facade is
   GLOBAL — no `&Logger` is threaded through functions, no DI. A call site is
   ONE line (`log::debug!("x={x}")`) that captures its locals. The bloat would
   be passing a logger everywhere; the facade avoids it.
2. **Instrument by need, not big-bang.** Add log calls where they earn their
   keep — hot paths, errors, state transitions, boundaries — incrementally.

## Front: reuse the facade macros (NO custom macros)
`log::{trace,debug,info,warn,error}!` already do the level-check BEFORE
formatting → disabled = zero evaluation/allocation, call sites stay tiny.
Writing custom macros = reinvention + bloat. Optional `target:` token groups
a call into a logical namespace.

## Namespaces = targets
- **Default target = the module path** (`shamir_engine::tx::commit`) — a
  hierarchical namespace for free, nothing to name.
- **Explicit target for cross-cutting concerns:** `log::debug!(target:
  "shomer", "...")`. The ONLY "naming" we design is a small canonical set so
  masks are predictable (a documented const list, not code at call sites):

  | target | subsystem |
  |---|---|
  | `shomer` | access / permissions |
  | `wal` | write-ahead log |
  | `tx` | transactions / commit pipeline |
  | `storage` | KV backends |
  | `engine` | table manager / general engine |
  | `query` | planner / batch executor |
  | `vector` | HNSW / brute-force vector |
  | `fts` | full-text search |
  | `fn` | function engine |
  | `auth` | SCRAM / sessions / RBAC |
  | `wire` | connect / protocol / transports |
  | `server` | ServerLauncher / lifecycle |
  | `migration` | online migration |

  Module-path targets remain the fallback for everything not grouped.

## Per-namespace levels by mask (the requested feature)
A directive-based filter (RUST_LOG-style): a default level + per-target
overrides matched by PREFIX (and `*`-glob, reusing `EnvPolicy`'s matcher):
```
default=info, shomer=trace, shamir_engine::vector=debug, wal=warn, wire=off
```
- Stored in `ArcSwap<TargetFilter>` → lock-free read on the hot `enabled()`
  path + **runtime reload** (bump `shomer=trace` on a live server with no
  restart). The filter is consulted in `Log::enabled(target, level)` → a
  masked-off namespace costs ZERO (no format, no alloc), same as a disabled
  level.
- Reload trigger: an admin op (`set_log_level`) and/or a signal (SIGHUP) —
  swaps the `ArcSwap`.

## Back: one `BatchLogger` (all the cleverness lives here)
A `log::Log` impl:
1. `enabled(meta)` → consult the `ArcSwap<TargetFilter>` (prefix/glob match).
2. `log(record)` (only reached when enabled) → render the line (one alloc;
   poolable via thread-local buffer) and `try_send` into a BOUNDED lock-free
   queue (`flume`/`crossbeam-channel`). Non-blocking → caller returns at once;
   no I/O on any hot path (async worker or engine).
3. A dedicated **writer thread** drains the queue, batches records, and
   flushes when `batch_size` reached OR `flush_interval` elapsed OR on
   shutdown — one write per batch.
4. **Overflow = drop + counter** (periodically emit "dropped N") — never
   block the producer. Policy configurable (drop-new / drop-oldest / buffer).

## Slices (each = one agent delegation; zero-trust + green gate)

### ✅ L-1 — Non-blocking stdout writer
- `tracing_appender::non_blocking` wrapping stdout.
- `init(&LoggingConfig)` returns a `LogGuard` the caller keeps alive.
- Lines dropped on overflow (lossy channel).

### ✅ L-2 — Batched file writer
- Bounded MPSC channel drained by one worker thread.
- `BufWriter<File>` with 256 KiB capacity.
- Flush on burst threshold or `flush_interval_ms` timer.
- Clean shutdown: guard drops shutdown sender → worker drains + flushes + exits.

### ✅ L-3 — Namespace masks + lock-free runtime reload
- **Namespace taxonomy** (`logging::ns`): 13 curated `const &str` targets
  (`ns::WAL`, `ns::ENGINE`, `ns::WIRE`, etc.) for cross-cutting concerns.
  Module-path targets are matched too — the mask works for any target string.
- **`LogMask`** — pure, testable decision type: a default `LevelFilter` plus
  a small `Vec<(String, LevelFilter)>` override table. `allows(target, level)`
  does a longest-prefix match; the effective level is compared to the event
  level. No tracing `Metadata` needed for testing.
- **Lock-free runtime handle** — a process-global `ArcSwap<LogMask>` behind
  `once_cell::sync::Lazy`. The hot-path decision is a single `ArcSwap::load`
  (one atomic read) + linear scan of the small override table. No
  `std::sync::Mutex`, `RwLock`, `parking_lot::*`, or
  `tracing_subscriber::reload` (which internally uses an `RwLock`).
  - `set_mask(LogMask)` — RCU swap the whole mask.
  - `set_namespace_level(target, LevelFilter)` — load → clone → override →
    store; observable without restart.
  - `current_mask()` — lock-free snapshot via `load_full()`.
- **Subscriber integration** — a `MaskFilter` implementing
  `tracing_subscriber::layer::Filter` whose `enabled()` reads the global
  `ArcSwap` and delegates to `LogMask::allows`. Composed via
  `registry().with(fmt_layer.with_filter(mask_filter))`. Boot default comes
  from `LoggingConfig::level`.
- **Convention**: log sites SHOULD use `tracing::info!(target: ns::WAL, …)`.
  Existing sites are NOT bulk-rewritten — that's out of scope; the mask
  matches any target string.

### 🔜 L-4 — Integration follow-ups
- Wire admin-op / SIGHUP trigger to call `set_namespace_level` live.
- Join the writer thread on graceful shutdown (GS milestone).
- Sink per run-mode (stdout / Event Log / file) from RUNTIME_MODES.
- Apply canonical `target:` groupings at the highest-value sites (commit,
  shomer, wal, wire) — by need, not big-bang.

## log vs tracing
- `log` + our `ArcSwap` filter: minimal churn (call sites unchanged),
  full control, fits the lock-free ethos.
- `tracing` + `tracing-subscriber::EnvFilter` + `tracing-appender::
  non_blocking`: structured fields + spans + per-target reloadable filter +
  a ready non-blocking batched writer — the gold standard for a DB, at the
  cost of migrating call sites (with `tracing-log` bridging existing `log`).
Recommendation: start on `log` + custom backend; adopt `tracing` if/when
structured logging (request spans, typed fields) is needed.

## Acceptance
Caller never blocks on log I/O; disabled level/namespace = zero alloc;
batched writes per config; per-namespace masks with live reload; clean flush
on shutdown. Gate green throughout.

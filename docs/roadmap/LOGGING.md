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
- **L-1 — BatchLogger backend**: the `Log` impl + bounded queue + writer
  thread + batch flush (size/interval) + overflow=drop+count. `init(config)`
  sets the global logger + spawns the thread + returns a guard. Tests:
  many messages don't block the producer; batches flush by size and by
  interval; overflow drops + counts; the guard flushes+joins on drop.
- **L-2 — Namespaces + mask filter + reload**: `TargetFilter` (default +
  per-target prefix/glob directives), `ArcSwap` storage, `enabled()` consults
  it, a parser for the directive string, a runtime `reload(directives)`.
  The canonical target taxonomy as consts + doc. Tests: prefix/glob match;
  masked-off = `enabled()` false (zero-cost); reload changes behavior live.
- **L-3 — Integration**: join the writer thread on graceful shutdown
  (GS milestone); sink per run-mode (stdout / Event Log / file) from
  RUNTIME_MODES; the `set_log_level` admin op (+ optional SIGHUP). Apply a
  handful of canonical `target:` groupings at the highest-value sites
  (commit, shomer, wal, wire) — by need, not big-bang.

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

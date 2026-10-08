<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-server — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Several narrow observations are valid, but the universal lock-free/admin-only characterization is inaccurate. The recorded log-mask lost-update race remains present.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 1 | 0 | 0 | 1 | 1 | 3 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-summary"></a>

### Claim Summary — All hot structures are lock-free; mutexes are exclusively admin/DDL/boot frequency

Status: `refuted`. Current risk: —.

DashMap paths explicitly hold shard locks, interactive transactions hold an async mutex across engine awaits, and audit appender mutexes serve authentication events. These are not all admin/boot paths. Whether pinned scc internals satisfy stronger lock-free claims was not verified.

Evidence: [crates/shamir-server/src/cursor_registry.rs:418](../../../../../crates/shamir-server/src/cursor_registry.rs#L418); [crates/shamir-server/src/conn_limiter.rs:233](../../../../../crates/shamir-server/src/conn_limiter.rs#L233); [crates/shamir-server/src/db_handler/tx_handlers.rs:165](../../../../../crates/shamir-server/src/db_handler/tx_handlers.rs#L165); [crates/shamir-server/src/audit_appender.rs:691](../../../../../crates/shamir-server/src/audit_appender.rs#L691); [crates/shamir-server/src/connection/handshake.rs:50](../../../../../crates/shamir-server/src/connection/handshake.rs#L50); [Cargo.lock:3123](../../../../../Cargo.lock#L3123).

<a id="review-notes-tables-registry"></a>

### Claim Notes/tables_registry — TablesRegistry mutex covers DDL-only synchronous persistence

Status: `not-applicable`. Current risk: —.

add/remove synchronously lock, mutate and write_atomic without an await. This is a DDL persistence path, not evidence of a new per-record hot-path violation.

Evidence: [crates/shamir-server/src/tables_registry.rs:139](../../../../../crates/shamir-server/src/tables_registry.rs#L139); [crates/shamir-server/src/tables_registry.rs:148](../../../../../crates/shamir-server/src/tables_registry.rs#L148); [crates/shamir-server/src/tables_registry.rs:159](../../../../../crates/shamir-server/src/tables_registry.rs#L159).

<a id="review-notes-set-namespace-level"></a>

### Claim Notes/set_namespace_level — Concurrent namespace updates can clobber each other

Status: `confirmed-open`. Current risk: `low`.

Both callers can load the same mask, independently clone/update it, then store; the later store loses the other's update. Operator-facing frequency limits impact but does not make the race correct. SIGHUP reload instead replaces the whole mask.

Evidence: [crates/shamir-server/src/logging.rs:168](../../../../../crates/shamir-server/src/logging.rs#L168); [crates/shamir-server/src/logging.rs:171](../../../../../crates/shamir-server/src/logging.rs#L171); [crates/shamir-server/src/logging.rs:377](../../../../../crates/shamir-server/src/logging.rs#L377).

<a id="review-notes-active-count"></a>

### Claim Notes/active_count — Supervisor len() is explicitly acknowledged off-hot-path telemetry

Status: `not-applicable`. Current risk: —.

The allow annotation and O(N) acknowledgement remain. The returned value counts registered subscriptions, not necessarily live tasks.

Evidence: [crates/shamir-server/src/replication/supervisor.rs:176](../../../../../crates/shamir-server/src/replication/supervisor.rs#L176); [crates/shamir-server/src/replication/supervisor.rs:178](../../../../../crates/shamir-server/src/replication/supervisor.rs#L178); [crates/shamir-server/src/replication/supervisor.rs:183](../../../../../crates/shamir-server/src/replication/supervisor.rs#L183).

<a id="review-notes-by-session-len"></a>

### Claim Notes/by_session_len — Cursor by_session_len is test-only

Status: `not-applicable`. Current risk: —.

The DashMap cardinality probe remains cfg(test)-gated; production callers use the per-session atomic count.

Evidence: [crates/shamir-server/src/cursor_registry.rs:705](../../../../../crates/shamir-server/src/cursor_registry.rs#L705); [crates/shamir-server/src/cursor_registry.rs:707](../../../../../crates/shamir-server/src/cursor_registry.rs#L707).

<a id="review-summary-no-lock-across-await"></a>

### Claim Summary/no-lock-across-await — No lock-across-await violations or unacknowledged hot-path scc len() calls

Status: `unverified`. Current risk: —.

The inspected len() exception is acknowledged and visible async mutex sites are documented, but a crate-wide universal absence claim requires more than the assigned-claim inspection. The production source is shared across multiple pull-stream tasks, so its single-task contention rationale is not universally established.

Evidence: [crates/shamir-server/src/replication/prod_factory.rs:79](../../../../../crates/shamir-server/src/replication/prod_factory.rs#L79); [crates/shamir-server/src/replication/prod_factory.rs:102](../../../../../crates/shamir-server/src/replication/prod_factory.rs#L102); [crates/shamir-server/src/replication/supervisor.rs:271](../../../../../crates/shamir-server/src/replication/supervisor.rs#L271); [crates/shamir-server/src/replication/supervisor.rs:275](../../../../../crates/shamir-server/src/replication/supervisor.rs#L275).

## Corrections and qualified non-findings

- Distinguish lock-free snapshot/counter operations from sharded locking and sanctioned async serialization.
- Do not describe audit-appender locking as exclusively admin/DDL/boot frequency.
- The acknowledged namespace lost-update race is a real low-impact operator-state defect, even though the original report declined to count it.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-server -- Concurrency & lock-free invariants

## Summary

`shamir-server` follows CLAUDE.md's five-pillar concurrency ideology closely and
consistently. Every hot-path structure (subscription registry/bridge, decode/deliver
caches, cursor/tx registries, connection limiters, byte budget, request loop) uses
`scc::HashMap`/`scc::TreeIndex`/`DashMap` with `THasher`, atomics, or `ArcSwap`, with
detailed inline comments justifying lock-ordering and closing prior races (several
documented past incidents: #1073, #1077, F-9/F-20 cursor reap races). The
`std::sync::Mutex`/`parking_lot::Mutex` instances that do exist are all admin/DDL/boot
frequency (`ServerMetaStore`, `FjallUserDirectory`, `TablesRegistry`, `FjallAuditAppender`)
and fit CLAUDE.md's sanctioned-exception categories. No lock-across-`.await` violations
and no un-acked O(N) `scc::*::len()` calls were found on any hot path.

## Findings

No findings for this theme.

Notes for completeness (not rising to reportable findings):

- `crates/shamir-server/src/tables_registry.rs:139-154` (`TablesRegistry::add`/`remove`)
  hold a `parking_lot::Mutex` across a synchronous temp-file write + rename
  (`write_atomic`). This is DDL-frequency only (fires once per `CreateTable`/`DropTable`),
  matches the sanctioned "DDL-only guard set" category, and does not block any read/write
  hot path — not worth a fix.
- `crates/shamir-server/src/logging.rs:168-172` (`set_namespace_level`) does a
  load-clone-with_override-store RCU without a CAS retry loop, so two concurrent callers
  can race and one override can silently clobber the other's. This is an operator-facing
  runtime log-level knob (SIGHUP-triggered), not a data-path concern, and the existing
  `scc`-registry code in this same crate (`SubscriptionRegistry::try_reserve`,
  `PerIpLimiter::try_acquire`) shows the team already knows how to CAS-loop when it
  matters — this one just isn't a hot path, so it wasn't flagged as a defect.
- `crates/shamir-server/src/replication/supervisor.rs:176-179`
  (`SubscriptionSupervisor::active_count`) uses `scc::HashMap::len()` (O(N)) but carries
  the required `#[allow(clippy::disallowed_methods)] // O(N) ack: test/telemetry, not hot
  path` comment per CLAUDE.md's rule — correctly acked, not a violation.
- `crates/shamir-server/src/cursor_registry.rs:705-708` (`CursorRegistry::by_session_len`)
  uses `DashMap::len()` (O(N) but not on `clippy.toml`'s banned list, since only
  `scc::*::len()` is banned) and is explicitly `#[cfg(test)]`-gated — correctly scoped,
  documented as such in its own doc comment.

</details>

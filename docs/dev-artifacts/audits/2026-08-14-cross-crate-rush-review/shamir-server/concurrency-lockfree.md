<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-server — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The namespace update race remains. The sharded/bucket-lock counter-evidence is stronger than the current report: exact scc source also contradicts universal lock-free entry access.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 1 | 0 | 0 | 1 | 1 | 3 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-summary"></a>

### Claim Summary — All hot structures are lock-free; mutexes are exclusively admin/DDL/boot frequency

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Audit appender locks process authentication events; DashMap takes shard locks; interactive transactions take async mutexes. Exact scc 3.8.4 published src/hash_map.rs explicitly uses bucket read/write locks for entry access, not universally lock-free operations.

Evidence: [crates/shamir-server/src/audit_appender.rs:691](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/audit_appender.rs#L691); [crates/shamir-server/src/conn_limiter.rs:238](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/conn_limiter.rs#L238); [crates/shamir-server/src/db_handler/tx_handlers.rs:165](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L165); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="review-notes-tables-registry"></a>

### Claim Notes/tables_registry — TablesRegistry mutex covers DDL-only synchronous persistence

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The inspected add/remove callers persist table lifecycle changes. Their mutex covers synchronous serialization/write/rename, with no await. That supports the narrow frequency/locking observation, not a durability or zero-worker-interference guarantee.

Evidence: [crates/shamir-server/src/tables_registry.rs:139](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tables_registry.rs#L139); [crates/shamir-server/src/tables_registry.rs:159](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tables_registry.rs#L159); [crates/shamir-server/src/db_handler/handler.rs:675](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/handler.rs#L675).

<a id="review-notes-set-namespace-level"></a>

### Claim Notes/set_namespace_level — Concurrent namespace updates can clobber each other

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Two callers can load mask M, independently add different overrides, then store M+A and M+B; the later store loses the other update. ArcSwap protects publication, not this compound read-modify-write. SIGHUP reload deliberately replaces the mask and is not this setter.

Evidence: [crates/shamir-server/src/logging.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging.rs#L168); [crates/shamir-server/src/logging.rs:171](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging.rs#L171); [crates/shamir-server/src/logging.rs:377](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging.rs#L377).

<a id="review-notes-active-count"></a>

### Claim Notes/active_count — Supervisor len() is explicitly acknowledged off-hot-path telemetry

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The allow annotation and scan acknowledgement exist. The count measures registry entries, not live follower tasks. scc 3.8.4 HashMap::len scans bucket counts rather than iterating every entry.

Evidence: [crates/shamir-server/src/replication/supervisor.rs:176](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L176); [crates/shamir-server/src/replication/supervisor.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L178); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="review-notes-by-session-len"></a>

### Claim Notes/by_session_len — Cursor by_session_len is test-only

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The helper is cfg(test)-gated and checks actual map cardinality. Production per-session admission uses the atomic counter.

Evidence: [crates/shamir-server/src/cursor_registry.rs:705](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/cursor_registry.rs#L705); [crates/shamir-server/src/cursor_registry.rs:415](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/cursor_registry.rs#L415).

<a id="review-summary-no-lock-across-await"></a>

### Claim Summary/no-lock-across-await — No lock-across-await violations or unacknowledged hot-path scc len() calls

Status: `unverified`. Current risk: `—`.

Prior-cycle decision: `unverified`.

Inspected async mutex sites intentionally span awaits and the cited len exception is acknowledged. A universal absence claim is not proved. In particular, a subscription shares one source among multiple pull-stream tasks, contradicting both source wrappers' single-task/uncontended rationale.

Evidence: [crates/shamir-server/src/replication/supervisor.rs:271](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L271); [crates/shamir-server/src/replication/supervisor.rs:282](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L282); [crates/shamir-server/src/replication/prod_factory.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/prod_factory.rs#L102); [crates/shamir-server/src/replication/wire_source.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/wire_source.rs#L67).

## Evidence and recipe corrections

- scc 3.8.4 was available in the cached published archive. Its HashMap entry operations use bucket read/write locks; the earlier dependency-unavailability limitation is obsolete.
- The root description that every scc len is iter().count() is inaccurate: HashMap sums bucket lengths across current/old arrays, while TreeIndex counts its iterator.
- WireReplSource serializes full pull awaits for multiple profile streams; 'single-tasked, uncontended' is not universally true.
- TablesRegistry's rename observation does not independently establish crash durability; write_atomic uses flush, not an explicit file/directory sync.

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

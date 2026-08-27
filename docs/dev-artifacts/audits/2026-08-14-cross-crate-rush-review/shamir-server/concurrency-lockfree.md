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

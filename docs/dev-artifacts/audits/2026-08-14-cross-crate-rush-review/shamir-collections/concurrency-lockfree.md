<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-collections — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The local concurrency non-findings remain valid: no locks, awaits, concurrent registries or synchronization state; all aliases and constructors explicitly select THasher. The capacity-constructor cost guarantee needs qualification.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 0 | 0 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

No local defect was established for this lens. The scope and positive-assurance qualifications below still apply.

## Corrections and qualified non-findings

- The crate-wide allow remains explicitly sanctioned by clippy.toml:39; narrowing it is optional lint containment, not remediation of a present concurrency violation.
- The dependency-light guest-facing design still explains the absence of scc/DashMap constructors.
- No concurrency-specific tests are required by a nonexistent local synchronization surface, but this does not make constructor and serialization contracts untestable.
- Do not describe every constructor as constant-time: pinned IndexMap documents nonzero capacity construction as O(capacity), and its core allocates both an indices table and an entries vector.
- RandomState cannot back a value of the exported aliases, but absence of randomization is not an input-trust or HashDoS-safety guarantee.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-collections — Concurrency & lock-free invariants

## Summary

Reviewed in full: `crates/shamir-collections/Cargo.toml` and `src/lib.rs` (64 lines) — these are the crate's only files; there is no `tests/`, `benches/`, or submodule tree. The crate is a pure type-alias/constructor leaf over `IndexMap` and std collections: it contains zero locks (no `std::sync::Mutex`/`RwLock`, no `parking_lot` — neither imported nor present in `[dependencies]`, which lists only `indexmap` and `rustc-hash`), zero `.await`s, and zero `scc`/`dashmap` usages, so none of the lens failure modes (hot-path lock, lock across `.await`, O(N) `scc::*::len()` without ack) can occur. Against the five pillars it is actively compliant rather than merely silent: `THasher = BuildHasherDefault<FxHasher>` pins every exported structure at the type level (`TMap`/`TSet`/`TFxMap`/`TFxSet` hard-code `THasher` as the hasher parameter, so `RandomState` is unreachable through this API) and at every constructor (`with_hasher(THasher::default())` / `with_capacity_and_hasher` — the explicitly-hashed forms clippy.toml whitelists), while all helpers are O(1)/O(capacity) preallocations with no hidden O(N) work. Two notes checked and cleared: (a) the crate-level `#![allow(clippy::disallowed_types)]` (`lib.rs:9`) is not a lint-masking violation — `clippy.toml` (lines 39–40) documents it verbatim as "The ONE sanctioned allow-site" for the banned std collection types, because those raw types appear only inside aliases that immediately inject `THasher`; (b) the absence of `scc`/`DashMap` convenience constructors from the Fx-hashing home crate is consistent with its documented design intent ("dependency-light leaf … guest-facing"), not a pillar gap. No tests exist under the crate, but there is no concurrency surface here whose behavior would need testing; general test-organization judgment is a sibling reviewer's theme.

## Findings

No findings for this theme.

</details>

<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-collections — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The local synchronization non-finding is supported. Collections are ordinary owned values, not concurrent registries; delegated allocation costs require separate qualification.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 0 | 0 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

## Evidence and recipe corrections

- No local locks, awaits, shared counters or concurrent registry mechanisms were found; adding scc/DashMap dependencies is not required for this guest-facing leaf.
- Nonzero ordered capacity construction is O(capacity) and uses two backing allocations, not universally constant-time.
- Type-level exclusion of RandomState establishes builder selection, not HashDoS resistance or trusted ingress.
- The sanctioned broad lint allow is optional containment debt, not a current concurrency-policy violation.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-collections — Concurrency & lock-free invariants

## Summary

Reviewed in full: `crates/shamir-collections/Cargo.toml` and `src/lib.rs` (64 lines) — these are the crate's only files; there is no `tests/`, `benches/`, or submodule tree. The crate is a pure type-alias/constructor leaf over `IndexMap` and std collections: it contains zero locks (no `std::sync::Mutex`/`RwLock`, no `parking_lot` — neither imported nor present in `[dependencies]`, which lists only `indexmap` and `rustc-hash`), zero `.await`s, and zero `scc`/`dashmap` usages, so none of the lens failure modes (hot-path lock, lock across `.await`, O(N) `scc::*::len()` without ack) can occur. Against the five pillars it is actively compliant rather than merely silent: `THasher = BuildHasherDefault<FxHasher>` pins every exported structure at the type level (`TMap`/`TSet`/`TFxMap`/`TFxSet` hard-code `THasher` as the hasher parameter, so `RandomState` is unreachable through this API) and at every constructor (`with_hasher(THasher::default())` / `with_capacity_and_hasher` — the explicitly-hashed forms clippy.toml whitelists), while all helpers are O(1)/O(capacity) preallocations with no hidden O(N) work. Two notes checked and cleared: (a) the crate-level `#![allow(clippy::disallowed_types)]` (`lib.rs:9`) is not a lint-masking violation — `clippy.toml` (lines 39–40) documents it verbatim as "The ONE sanctioned allow-site" for the banned std collection types, because those raw types appear only inside aliases that immediately inject `THasher`; (b) the absence of `scc`/`DashMap` convenience constructors from the Fx-hashing home crate is consistent with its documented design intent ("dependency-light leaf … guest-facing"), not a pillar gap. No tests exist under the crate, but there is no concurrency surface here whose behavior would need testing; general test-organization judgment is a sibling reviewer's theme.

## Findings

No findings for this theme.

</details>

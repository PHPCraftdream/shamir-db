<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-bench-utils — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

No production crypto or remote-input vulnerability is established. The allocator remains a low-severity, process-wide development-tooling side effect.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Feature-gated `#[global_allocator]` in a library crate — process-wide side effect

Status: `confirmed-open`. Current risk: `low`.

Enabling peak_mem still places a global allocator in the library. Both current dependency edges are dev-only, but linked fixture-only benches inherit the allocator too. No existing production dependency edge or secret-handling use was found.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:39](../../../../../crates/shamir-bench-utils/src/peak_mem.rs#L39); [crates/shamir-engine/Cargo.toml:107](../../../../../crates/shamir-engine/Cargo.toml#L107); [crates/shamir-index/Cargo.toml:64](../../../../../crates/shamir-index/Cargo.toml#L64); [crates/shamir-engine/benches/filtered_vector_search.rs:25](../../../../../crates/shamir-engine/benches/filtered_vector_search.rs#L25).

Grouping/duplicate: `SUMMARY.md#5.1`. This row is not another independent defect.

## Corrections and qualified non-findings

- Own src contains no unsafe/static mut, auth/HMAC/SCRAM/TLS, file/network/process/environment operations, or secret material; this does not audit dependency implementations.
- Lcg is explicitly non-cryptographic and has only fixture uses. Current ordinary production dependency paths do not include this crate; that is a current graph fact, not an architectural prohibition on future regular dependencies.
- The checksummed peak_alloc 0.3.0 lockfile pin is confirmed at Cargo.lock:2396; pinning is not evidence of dependency behavioral or memory-safety guarantees.
- A bench-local allocator macro/snippet is only a design option: measurement helpers must access that same allocator's counters.
- No compiler diagnostic number or quantitative allocator-overhead claim was verified.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-bench-utils -- Security & crypto boundary

## Summary

This crate sits entirely outside the crypto boundary: it contains no auth/HMAC/SCRAM/TLS
code, zero `unsafe`, zero `static mut`, and no file/network/process/env surface
(grep-verified across all of `src/`). Its only randomness is `Lcg`
(`vector_data.rs:52-104`), explicitly documented as **not** cryptographically secure
(`vector_data.rs:32-38`) and used solely for deterministic bench fixture data; both
workspace consumers declare it under `[dev-dependencies]` only (`shamir-engine/Cargo.toml:107`,
`shamir-index/Cargo.toml:64`), so neither the LCG nor the feature-gated allocator swap can
reach a production build (shamir-index's tests deliberately re-implement the LCG rather than
import it, further confirming the library path never touches it). No injection surface (no
commands, paths, queries, or env reads) and no timing-side-channel surface exist — the crate
holds no secrets, and `peak_alloc` is pinned at 0.3.0 with a registry checksum in
`Cargo.lock:2395-2399`. One low-severity hygiene finding: the `peak_mem` feature installs a
process-global allocator from a *library* crate.

## Findings

### 1. Feature-gated `#[global_allocator]` in a library crate — process-wide side effect

- **File:line:** `crates/shamir-bench-utils/src/peak_mem.rs:39-40` (exported via `src/lib.rs:14-15`)
- **Severity:** low
- **Issue:** Enabling the `peak_mem` cargo feature compiles a `#[global_allocator]`
  (`PeakAlloc`) into the library itself. An allocator is a *process-global* property of the
  final binary, so any binary that links `shamir-bench-utils` with this feature enabled gets
  the tracking allocator wrapped around **every** allocation in the process — including code
  completely unrelated to measurement. The module doc ("normal `cargo bench` paths are
  unaffected", `peak_mem.rs:3-4`) understates this: the effect is not scoped to benches that
  call `setup()`, it is process-wide the moment the feature is on.
- **Failure scenario:** None today. Both consumers gate it behind `[dev-dependencies]`, and a
  binary that declares its own global allocator fails loudly at compile time (E0159,
  duplicate `#[global_allocator]`). The risk is drift: if a future runtime dependency (or a
  released example/binary) enables `peak_mem`, a production binary silently inherits an
  allocation-tracking allocator — per-alloc atomic overhead and global counters contended by
  all threads — with no build-time signal.
- **Suggested fix:** Keep the allocator out of the library surface: e.g. export a small
  `declare_peak_alloc!()` macro (or a documented snippet) that each *bench binary* pastes, so
  `#[global_allocator]` lives in the bench file, not the library. Alternatively, at minimum
  extend the `peak_mem` module doc to state the process-wide implication, the E0159
  interaction with binaries that have their own allocator, and an explicit "dev-dependency
  only" warning.

No other findings for this theme.

</details>

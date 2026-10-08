<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-bench-utils — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

No crypto, secret, or remote-input mechanism is present in own source. The allocator issue is current development-tooling scope; extrapolation to all production configurations or dependency memory safety would exceed the evidence.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Feature-gated `#[global_allocator]` in a library crate — process-wide side effect

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The allocator is declared in the library and enabled through current dev dependencies, including linked fixture-only benches. No normal production dependency or secret-related use was found. A future regular edge is a prospective configuration risk, not an existing exploit.

Evidence: [crates/shamir-bench-utils/src/peak_mem.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-bench-utils/src/peak_mem.rs#L39); [crates/shamir-engine/Cargo.toml:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/Cargo.toml#L107); [crates/shamir-index/Cargo.toml:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/Cargo.toml#L64); [crates/shamir-engine/benches/filtered_vector_search.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/benches/filtered_vector_search.rs#L25).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

## Evidence and recipe corrections

- The own-source no-unsafe/no-secret result is supported, but peak_alloc itself implements unsafe GlobalAlloc methods; dependency safety is a separate obligation.
- Current dev-only edges exclude this helper from ordinary production dependency paths, not every possible future or explicitly shipped example configuration.
- No duplicate-allocator diagnostic number or numerical overhead was verified.
- A bench-local declaration can retain shared peak_alloc counters, but a replacement allocator design must make helpers read the allocator actually installed in that binary.

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

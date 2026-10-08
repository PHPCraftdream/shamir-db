<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-numa — error-handling-lifecycle revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Unbounded range expansion, blanket per-node error swallowing, missing deterministic Linux failure tests, and silent degradation remain. CPU_SET outcomes lack exact dependency proof; Unsupported remains context-free.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 6 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- Exact Linux libc helper source proves checked-index bounds panic; remove contradictory glibc/musl C-macro outcomes.

<a id="review-1"></a>

### Claim 1 — `parse_cpulist` expands unbounded ranges -- malformed input can panic/abort the process

Status: `confirmed-open`. Current risk: `medium`.

A valid ascending range expands without checked span or output limits; best-effort token handling does not prevent excessive allocation. Exact reserve strategy, failure timing, and usize-max examples depend on target/toolchain.

Evidence: [crates/shamir-numa/src/cpulist.rs:20](../../../../../crates/shamir-numa/src/cpulist.rs#L20); [crates/shamir-numa/src/cpulist.rs:38](../../../../../crates/shamir-numa/src/cpulist.rs#L38); [crates/shamir-numa/src/cpulist.rs:40](../../../../../crates/shamir-numa/src/cpulist.rs#L40); [crates/shamir-numa/src/tests/cpulist_tests.rs:47](../../../../../crates/shamir-numa/src/tests/cpulist_tests.rs#L47).

<a id="review-2"></a>

### Claim 2 — `probe()` swallows every per-node cpulist I/O error, contradicting its doc and yielding a silently broken topology

Status: `confirmed-open`. Current risk: `medium`.

Err(_) still swallows Syscall as well as Unsupported. An all-empty discovered topology is accepted and detect() returns it because node count, not total usable CPUs, is checked.

Evidence: [crates/shamir-numa/src/linux.rs:50](../../../../../crates/shamir-numa/src/linux.rs#L50); [crates/shamir-numa/src/linux.rs:78](../../../../../crates/shamir-numa/src/linux.rs#L78); [crates/shamir-numa/src/linux.rs:92](../../../../../crates/shamir-numa/src/linux.rs#L92); [crates/shamir-numa/src/linux.rs:170](../../../../../crates/shamir-numa/src/linux.rs#L170); [crates/shamir-numa/src/detect.rs:26](../../../../../crates/shamir-numa/src/detect.rs#L26).

<a id="review-3"></a>

### Claim 3 — `CPU_SET` without a `CPU_SETSIZE` bound: silent mask truncation (glibc) / out-of-bounds write (musl)

Status: `confirmed-open`. Current risk: `medium`.

Parent inspection of checksummed libc 0.2.186 resolves the Linux helper: cpu_set_t has 1024 storage bits, and CPU_SET indexes bits[cpu / word_bits] with Rust array indexing. A sysfs CPU ID >=1024 therefore reaches bounds panic, not a C-macro out-of-bounds write or silent truncation. The unchecked source route remains a Medium public Linux API defect; no production worker-pinning route or tested target failure was established.

Evidence: [crates/shamir-numa/src/linux.rs:139](../../../../../crates/shamir-numa/src/linux.rs#L139); [crates/shamir-numa/src/linux.rs:143](../../../../../crates/shamir-numa/src/linux.rs#L143); [crates/shamir-numa/src/linux.rs:147](../../../../../crates/shamir-numa/src/linux.rs#L147); [Cargo.lock:1923](../../../../../Cargo.lock#L1923).

Pinned dependency evidence: [libc 0.2.186, src/unix/linux_like/linux_l4re_shared.rs:1531](https://docs.rs/crate/libc/0.2.186/source/src/unix/linux_like/linux_l4re_shared.rs).

<a id="review-4"></a>

### Claim 4 — No error-path tests for the Linux probe layer; error branches never compiled on the primary gate

Status: `confirmed-open`. Current risk: `medium`.

There is still no injected-read seam or test asserting probe Unsupported/Syscall mapping. Linux cfg excludes these branches on Windows, but current Ubuntu CI does compile them; the broader CI-noncompilation implication is false.

Evidence: [crates/shamir-numa/src/linux.rs:167](../../../../../crates/shamir-numa/src/linux.rs#L167); [crates/shamir-numa/src/linux.rs:179](../../../../../crates/shamir-numa/src/linux.rs#L179); [crates/shamir-numa/src/lib.rs:47](../../../../../crates/shamir-numa/src/lib.rs#L47); [.github/workflows/numa.yml:29](../../../../../.github/workflows/numa.yml#L29); [.github/workflows/ci.yml:65](../../../../../.github/workflows/ci.yml#L65).

<a id="review-5"></a>

### Claim 5 — `detect()` degrades silently -- swallowed probe error has no observability

Status: `confirmed-open`. Current risk: `low`.

Linux detect() still discards the probe Err through if let Ok and returns fallback without recording a reason. No diagnostic-returning factory or logging path exists.

Evidence: [crates/shamir-numa/src/detect.rs:25](../../../../../crates/shamir-numa/src/detect.rs#L25); [crates/shamir-numa/src/detect.rs:30](../../../../../crates/shamir-numa/src/detect.rs#L30); [crates/shamir-numa/Cargo.toml:18](../../../../../crates/shamir-numa/Cargo.toml#L18).

<a id="review-6"></a>

### Claim 6 — `AffinityError::Unsupported` is overloaded and carries no source

Status: `confirmed-open`. Current risk: `nit`.

Unsupported remains a unit variant used for missing discovery files and empty online input. It cannot distinguish those conditions. The claimed live affinity-less-OS use is not present: built-in fallback pinning returns success or NodeOutOfRange.

Evidence: [crates/shamir-numa/src/error.rs:10](../../../../../crates/shamir-numa/src/error.rs#L10); [crates/shamir-numa/src/linux.rs:63](../../../../../crates/shamir-numa/src/linux.rs#L63); [crates/shamir-numa/src/linux.rs:170](../../../../../crates/shamir-numa/src/linux.rs#L170); [crates/shamir-numa/src/fallback.rs:54](../../../../../crates/shamir-numa/src/fallback.rs#L54).

## Corrections and qualified non-findings

- Confirmed error/lifecycle non-findings: thiserror supplies the library error enum and io::Error source conversion; fallible discovery/pinning return Result; there is no async destructor, task lifecycle, or external connection resource.
- The crate is not dependency-free: arc-swap, thiserror, and Linux libc/shamir-collections are declared. Missing a tracing dependency does not itself establish absent observability; the actual discarded error does.
- Rust Vec reservation details and maximum-usize examples should not be universalized across pointer widths or toolchains. Unbounded expansion is the source-proven mechanism.
- The Linux error branches are compiled by configured Ubuntu jobs, though their failure outcomes remain untested. No CI result was fetched or claimed.
- A pure cpu_set_t builder is still Linux-specific unless a platform-independent representation is extracted; CPU_ALLOC availability and dynamic-buffer invariants must be checked against the exact pin before recommending an implementation.
- Reading a file successfully but partially does not inherently produce an I/O error; replace the report's blanket truncated-read example with actual propagated read/UTF-8 errors.
- Passing an empty affinity mask loses discovery provenance, but exact kernel errno remains reference-unverified in this read-only pass.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-numa -- Error handling & resource lifecycle

## Summary

The crate largely honours CLAUDE.md's error ideology: one `thiserror` enum (`AffinityError`) with `#[from] std::io::Error`, `Result` on every fallible entry point, deliberate infallible read paths, and defensive clamping (`replica()`, `cores_on_node`, `current_node`) instead of indexing panics. The genuine gaps are concentrated on the Linux-only probe error paths: an over-broad `Err(_) =>` swallow in `probe()` that contradicts its own doc, an unbounded range expansion in the public `parse_cpulist` that turns malformed input into a process abort, and an unguarded `CPU_SET` against `CPU_SETSIZE`. None of the `probe()`/`fs_read_trim` error branches is tested anywhere, and none of them is even compiled by the primary Windows gate.

## Findings

### 1. `parse_cpulist` expands unbounded ranges -- malformed input can panic/abort the process
- File: `crates/shamir-numa/src/cpulist.rs:40` (range handling 36-42)
- Severity: medium
- Issue: `cpus.extend(lo..=hi)` reserves the whole range up front (`RangeInclusive<usize>` is `TrustedLen`, so `Vec::extend` allocates `(hi - lo + 1) * 8` bytes in one shot). There is no upper bound on `hi`. A token like `0-99999999999` attempts a ~800 GB reservation (allocation failure -> uncatchable abort); a token like `0-18446744073709551615` overflows the capacity computation (`capacity overflow` panic). This is the one malformed input in a function whose documented policy is otherwise "skip silently" (garbage tokens, reversed ranges), i.e. inconsistent with its own best-effort contract.
- Failure scenario: the function is `pub` and its docs explicitly advertise it "for tooling that inspects `/proc` / `/sys` cpu masks" -- i.e. strings that need not come from the kernel. A corrupt or hostile cpulist string fed by such tooling (or a future config surface) kills the whole process during a parse that is supposed to be best-effort.
- Suggested fix: bound the expansion (e.g. reject `hi - lo > MAX_REASONABLE_CPUS` or `hi >= 1 << 20`) and skip the offending token like other malformed input; add an error-path test (`huge_range_is_skipped`).

### 2. `probe()` swallows every per-node cpulist I/O error, contradicting its doc and yielding a silently broken topology
- File: `crates/shamir-numa/src/linux.rs:75-79` (doc at 50-51; contrast `fs_read_trim` at 167-173; fallback gate at `detect.rs:24-31`)
- Severity: medium
- Issue: the doc says only "A missing per-node `cpulist` is treated as an empty CPU list (best-effort)", but `Err(_) => Vec::new()` swallows *all* error kinds -- `EACCES` (hardened container), `EIO`, truncated reads -- the exact split `fs_read_trim` 20 lines earlier carefully encodes as `Unsupported` vs `Syscall`. A real I/O failure becomes a zero-CPU node indistinguishable from a legitimately empty one.
- Failure scenario: (a) `pin_current_thread_to_node` on such a node builds an empty `cpu_set_t`, `sched_setaffinity` fails with `EINVAL`, and the caller sees a baffling `AffinityError::Syscall` far from the root cause; (b) if every node's cpulist read fails while `online` read succeeded, `probe()` still returns `Ok` with `num_nodes() >= 1`, so `detect()` returns that broken topology instead of degrading to `FallbackSingleNodeTopology` -- it only falls back when `probe()` errors or reports zero nodes. `shamir-index` (`index_info.rs:142`+, `sorted_index_manager.rs:299-300`) already consumes `detect()` directly, so the broken topology flows into production registries.
- Suggested fix: match on the error kind -- `NotFound` -> empty vec (the documented best-effort case), anything else -> propagate with `?`; additionally have `detect()` degrade to the fallback when the probed topology owns zero CPUs in total.

### 3. `CPU_SET` without a `CPU_SETSIZE` bound: silent mask truncation (glibc) / out-of-bounds write (musl)
- File: `crates/shamir-numa/src/linux.rs:139-150` (loop at 142-144)
- Severity: medium
- Issue: `libc::CPU_SET(cpu.0, &mut cpu_set)` on a fixed 1024-bit `cpu_set_t` is silently ignored for `cpu.0 >= 1024` under glibc, and musl's `CPU_SET` macro has no bounds check at all (out-of-bounds stack write). The crate's own README plans an `x86_64-unknown-linux-musl` build for the QEMU tier. The SAFETY comment claims full initialisation/ABI correctness and never mentions the `CPU_SETSIZE` limit.
- Failure scenario: on a >1024-logical-CPU host (real: 4-socket SMT servers), a glibc build silently excludes the high CPUs of a node -- potentially producing an empty effective mask, `sched_setaffinity` -> `EINVAL`, and a misleading `Syscall` error with no hint that the mask was truncated; a musl build corrupts the stack.
- Suggested fix: size the mask dynamically with `libc::CPU_ALLOC` / `CPU_ALLOC_SIZE`, or explicitly check `cpu.0 < libc::CPU_SETSIZE` and return a descriptive error (`AffinityError::Syscall(io::Error::new(InvalidInput, ...))` or a dedicated variant) instead of silently mis-pinning; document the limit in the SAFETY block.

### 4. No error-path tests for the Linux probe layer; error branches never compiled on the primary gate
- File: `crates/shamir-numa/src/linux.rs:167-173` (`fs_read_trim` mapping), 52-93 (`probe` branches), 179-200 (inline tests, happy-path only)
- Severity: medium
- Issue: `AffinityError::Unsupported` and `AffinityError::Syscall` are never constructed or asserted in any test that runs on the CI matrix -- only `NodeOutOfRange` is (mock_tests, fallback_tests, both good). `fs_read_trim`'s NotFound-vs-other mapping, `probe()`'s empty-`online` -> `Unsupported` branch, and the per-node swallow from finding 2 are all untested: there is no seam to inject a missing/unreadable sysfs file (reads and parse are not separated, unlike the purely-tested `parse_cpulist`). The inline `#[cfg(all(test, target_os = "linux"))]` module (which also violates the "never embed `#[cfg(test)] mod tests` inline" rule -- sibling reviewers' theme) covers only happy paths against a real host, as does `tests/linux_topology.rs`. Because `linux.rs` is `cfg(target_os = "linux")`, none of these error branches is even compiled by the Windows dev-host gate that CLAUDE.md makes mandatory.
- Failure scenario: any regression in the error mapping -- flipping `Unsupported`/`Syscall` in `fs_read_trim`, or widening the per-node swallow further -- passes the entire suite unnoticed.
- Suggested fix: separate read from parse in the probe (take file contents or a tiny read trait), unit-test the mapping and both `probe()` failure branches platform-independently; add the huge-range cpulist test; keep the Linux-only tests in a `tests/` directory per the documented layout.

### 5. `detect()` degrades silently -- swallowed probe error has no observability
- File: `crates/shamir-numa/src/detect.rs:24-31`
- Severity: low
- Issue: `if let Ok(topo) = LinuxTopology::probe()` discards the error with no log. The workspace already standardises on `tracing` (shamir-server, shamir-client), but this crate depends on nothing, so a deployment that silently lost NUMA awareness (missing sysfs, probe `Syscall`) is indistinguishable from a genuine single-socket host. Returning a usable topology is fine; doing it invisibly is not.
- Failure scenario: NUMA-replicated index registries (`shamir-index` already calls `detect()` on construction paths) run degraded in production with zero trace evidence of why, and the misperformance is diagnosed only by archaeology.
- Suggested fix: emit `warn!`/`info!` (or at least record the probe error) when the Linux probe fails and the fallback is chosen; an optional `tracing` dependency suffices.

### 6. `AffinityError::Unsupported` is overloaded and carries no source
- File: `crates/shamir-numa/src/error.rs:10-16`
- Severity: nit
- Issue: the same variant covers "sysfs hierarchy missing" (from `probe`) and "platform genuinely has no affinity", while the doc asks callers to treat it as a soft no-op. With no `source` or context, a caller logging the error cannot distinguish "container without sysfs" from "kernel without NUMA support". The rest of the enum follows CLAUDE.md's thiserror/`#[from]` discipline exactly.
- Suggested fix: either attach context (`Unsupported { path: &'static str }`) or split a `SysfsUnavailable` variant, keeping `Unsupported` for the genuine platform case.

</details>

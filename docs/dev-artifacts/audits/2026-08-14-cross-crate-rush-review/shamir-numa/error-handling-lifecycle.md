<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-numa — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The fixed-mask panic violates a real fallible API contract. Parser expansion and discovery error erasure remain lower-severity, qualified issues; context-free Unsupported is intentional taxonomy rather than a required repair.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 5 | 0 | 0 | 0 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `parse_cpulist` expands unbounded ranges -- malformed input can panic/abort the process

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Valid ascending ranges and repeated tokens expand without budgets. The mechanism is present, but malformed-token skipping does not promise bounded handling of every valid range. Current trusted sysfs callers warrant lower severity than an established service-input DoS.

Evidence: [crates/shamir-numa/src/cpulist.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/cpulist.rs#L18); [crates/shamir-numa/src/cpulist.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/cpulist.rs#L38); [crates/shamir-numa/src/cpulist.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/cpulist.rs#L40).

<a id="review-2"></a>

### Claim 2 — `probe()` swallows every per-node cpulist I/O error, contradicting its doc and yielding a silently broken topology

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Err(_) discards Syscall errors beyond the documented missing-file exception. An online list with failed CPU-list reads yields an accepted all-empty topology. This proves provenance loss and degraded discovery, not demonstrated record corruption or production pin failure.

Evidence: [crates/shamir-numa/src/linux.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L50); [crates/shamir-numa/src/linux.rs:78](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L78); [crates/shamir-numa/src/linux.rs:92](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L92); [crates/shamir-numa/src/linux.rs:170](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L170); [crates/shamir-numa/src/detect.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/detect.rs#L26).

<a id="review-3"></a>

### Claim 3 — `CPU_SET` without a `CPU_SETSIZE` bound: silent mask truncation (glibc) / out-of-bounds write (musl)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

CPU ID 1024 makes libc 0.2.186 published linux_l4re_shared.rs:1531 index outside its 1024-bit storage. Pinning panics before returning Result. Both Linux GNU and musl use this Rust helper; the C truncation/corruption allegations are false. Reject or safely represent the unsupported domain.

Evidence: [crates/shamir-numa/src/linux.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L140); [crates/shamir-numa/src/linux.rs:143](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L143); [Cargo.lock:1923](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1923); [Cargo.toml:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.toml#L88).

<a id="review-4"></a>

### Claim 4 — No error-path tests for the Linux probe layer; error branches never compiled on the primary gate

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No production read seam allows deterministic NotFound/Syscall/sparse-node tests. Linux code is selected by configured Ubuntu jobs, contradicting blanket CI noncompilation. Mutating error classification would not be discriminated by the current happy-host assertions.

Evidence: [crates/shamir-numa/src/linux.rs:167](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L167); [crates/shamir-numa/src/linux.rs:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L179); [.github/workflows/numa.yml:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.github/workflows/numa.yml#L29); [.github/workflows/ci.yml:65](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.github/workflows/ci.yml#L65).

<a id="review-5"></a>

### Claim 5 — `detect()` degrades silently -- swallowed probe error has no observability

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

if let Ok discards the probe error before returning fallback. Infallible degradation is intentional and supported; the open issue is operational diagnostic loss, not a failure to return a usable topology.

Evidence: [crates/shamir-numa/src/detect.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/detect.rs#L11); [crates/shamir-numa/src/detect.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/detect.rs#L25); [crates/shamir-numa/src/detect.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/detect.rs#L30).

<a id="review-6"></a>

### Claim 6 — `AffinityError::Unsupported` is overloaded and carries no source

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The variant deliberately classifies unavailable discovery without promising a retained source. Missing files and empty online lists fit that taxonomy; built-in fallback pinning does not return it. More context is optional diagnostics/API design, not an error-contract violation.

Evidence: [crates/shamir-numa/src/error.rs:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/error.rs#L16); [crates/shamir-numa/src/linux.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L63); [crates/shamir-numa/src/linux.rs:170](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L170); [crates/shamir-numa/src/fallback.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/fallback.rs#L54).

## Evidence and recipe corrections

- The overview's unresolved-helper assertion conflicts with the parent refinement and exact published libc 0.2.186 source.
- libc 0.2.186 exposes CPU_ALLOC_SIZE but no Linux CPU_ALLOC function. Do not propose calling the C macro as an available Rust API.
- CPU_SETSIZE is not uniformly 1024 in that package: the musl constant is cfg-dependent, while the shared cpu_set_t storage is 1024 bits. Define the supported domain from the actual representation and target, not a universal constant assertion.
- A dynamic mask needs checked sizing, initialized word storage, alignment, correct byte length and lifetime through the syscall. Silently filtering unsupported CPUs does not implement a strict rejection contract.
- Keep legitimate memory-only/empty nodes distinct from failed discovery. An all-empty fallback policy should preserve diagnostics rather than pretend missing CPUs imply a genuine one-node host.
- Empty or disjoint permitted affinity masks can produce EINVAL; successful pinning remains within the permitted subset. The historical suggestion that omitted high CPUs could remain schedulable outside a successful mask is wrong. [Linux affinity semantics](https://man7.org/linux/man-pages/man2/sched_setaffinity.2.html).

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

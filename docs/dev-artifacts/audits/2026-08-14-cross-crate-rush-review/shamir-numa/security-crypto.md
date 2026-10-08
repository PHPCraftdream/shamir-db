<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-numa — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Unbounded parser expansion remains a conditional embedding DoS risk, not an established network vulnerability. Discovery diagnostic loss remains. The exact CPU_SET panic claim is unverified against the resolved libc source.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 3 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- Exact Linux libc helper source proves checked-index bounds panic; remove contradictory glibc/musl C-macro outcomes.

<a id="review-1"></a>

### Claim 1 — `CPU_SET` fed sysfs CPU indices with no `CPU_SETSIZE` bound -- panic instead of `Err` on >=1024-CPU hosts

Status: `confirmed-open`. Current risk: `medium`.

Parent inspection of checksummed libc 0.2.186 resolves the Linux helper: cpu_set_t has 1024 storage bits, and CPU_SET indexes bits[cpu / word_bits] with Rust array indexing. A sysfs CPU ID >=1024 therefore reaches bounds panic, not a C-macro out-of-bounds write or silent truncation. The unchecked source route remains a Medium public Linux API defect; no production worker-pinning route or tested target failure was established.

Evidence: [crates/shamir-numa/src/linux.rs:129](../../../../../crates/shamir-numa/src/linux.rs#L129); [crates/shamir-numa/src/linux.rs:143](../../../../../crates/shamir-numa/src/linux.rs#L143); [crates/shamir-numa/tests/linux_topology.rs:17](../../../../../crates/shamir-numa/tests/linux_topology.rs#L17); [Cargo.lock:1923](../../../../../Cargo.lock#L1923).

Pinned dependency evidence: [libc 0.2.186, src/unix/linux_like/linux_l4re_shared.rs:1531](https://docs.rs/crate/libc/0.2.186/source/src/unix/linux_like/linux_l4re_shared.rs).

Grouping/duplicate: `error-handling-lifecycle.md#3`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `parse_cpulist` expands ranges with no cap -- infallible public API aborts the process on large well-formed input

Status: `confirmed-open`. Current risk: `low`.

The public parser materializes unconstrained ascending ranges. A caller accepting hostile strings can cause excessive allocation; existing production feed points are trusted sysfs, not network/config inputs.

Evidence: [crates/shamir-numa/src/cpulist.rs:29](../../../../../crates/shamir-numa/src/cpulist.rs#L29); [crates/shamir-numa/src/cpulist.rs:40](../../../../../crates/shamir-numa/src/cpulist.rs#L40); [crates/shamir-numa/src/linux.rs:57](../../../../../crates/shamir-numa/src/linux.rs#L57); [crates/shamir-numa/src/linux.rs:76](../../../../../crates/shamir-numa/src/linux.rs#L76).

Grouping/duplicate: `error-handling-lifecycle.md#1`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — `probe` swallows all per-node cpulist read errors into an empty node -- later surfaces as a bare `EINVAL`

Status: `confirmed-open`. Current risk: `nit`.

Err(_) still converts every per-node read failure to an empty CPU list, losing the source error before affinity-mask construction. Discovery provenance is absent from later pin failures; no privilege escalation is established.

Evidence: [crates/shamir-numa/src/linux.rs:75](../../../../../crates/shamir-numa/src/linux.rs#L75); [crates/shamir-numa/src/linux.rs:78](../../../../../crates/shamir-numa/src/linux.rs#L78); [crates/shamir-numa/src/linux.rs:142](../../../../../crates/shamir-numa/src/linux.rs#L142); [crates/shamir-numa/src/linux.rs:153](../../../../../crates/shamir-numa/src/linux.rs#L153).

Grouping/duplicate: `error-handling-lifecycle.md#2`. This row is not another independent defect.

## Corrections and qualified non-findings

- Confirmed scoped non-findings: no authentication, cryptography, secrets, networking, command execution, or environment reads in this crate. Dynamic sysfs path segments are parsed usize values, not arbitrary path text.
- Numeric CPU keys from trusted discovery do not establish attacker-controlled HashDoS or secret-comparison timing exposure.
- An unsafe block containing CPU_SET does not establish memory corruption. C musl/glibc macro behavior must not be attributed to Rust helpers.
- Panic does not necessarily abort the process under an unwinding profile; exact helper behavior and deployment panic strategy were not demonstrated.
- The alleged startup worker-pinning scenario is hypothetical: the repository-wide caller search found no production use of this crate's pin API.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-numa -- Security & crypto boundary

## Summary

This crate exposes no authentication, crypto, or network surface at all -- no HMAC/SCRAM/TLS, no secrets, no command execution, no env reads; its entire security boundary is (a) the two `unsafe` libc calls in the Linux topology impl and (b) parsing of kernel-generated `/sys` cpulist text. The `unsafe` blocks are mostly well-annotated, but the `sched_setaffinity` block feeds sysfs-derived CPU indices into `libc::CPU_SET` without a `CPU_SETSIZE` bound (reachable panic on >=1024-CPU hosts), and the public `parse_cpulist` expands ranges with no cap (OOM abort on large well-formed input). Path construction interpolates only parsed `usize` values, so there is no path traversal or injection. Nothing secret is ever compared or hashed for lookup by attacker-controlled keys, so there is no timing side-channel surface.

## Findings

### 1. `CPU_SET` fed sysfs CPU indices with no `CPU_SETSIZE` bound -- panic instead of `Err` on >=1024-CPU hosts
- **File:line:** `crates/shamir-numa/src/linux.rs:139-150` (loop body `CPU_SET` at :143; `// SAFETY:` block :129-138)
- **Severity:** medium
- **Issue:** `pin_current_thread_to_node` iterates the node's CPU list (parsed verbatim from `/sys/devices/system/node/nodeN/cpulist`) and calls `libc::CPU_SET(cpu.0, ...)` for each index. `CPU_SETSIZE` is 1024 on Linux, and libc 0.2.x's Rust `CPU_SET` helper computes `cpuset.__bits[cpu / 64] |= 1 << (cpu % 64)` over a `[c_ulong; 16]` array -- any CPU index >= 1024 is out of bounds (debug builds trip the `debug_assert!`; release builds hit the bounds-checked array index and panic). On a host where the highest CPU number reaches 1024+ (dense 4/8-socket big iron; distro kernels configure `NR_CPUS` up to 8192; large VMs), any node whose cpulist contains a CPU >= 1024 makes the pin path panic -- a crash reachable purely from kernel-reported data, violating the crate's `Result`-based error contract (CLAUDE.md error handling: avoid `panic!` outside invariant violations).
- **Failure scenario:** 8-socket server where node 7 owns CPUs 3584-4095. Startup pins worker threads per node; pinning to node 7 panics ("index out of bounds: the len is 16 but the index is 56") inside the `unsafe` block and aborts the process. The `// SAFETY:` comment justifies zero-init, pointer validity, `cpusetsize`, and `pid = 0`, but omits this range precondition -- the documented soundness argument for the unsafe block is incomplete.
- **Suggested fix:** Filter (or explicitly reject) CPU indices >= `libc::CPU_SETSIZE as usize` before `CPU_SET` -- skip them under the same best-effort policy the parser uses, or return a new `AffinityError::CpuOutOfRange` variant. Extend the `// SAFETY:` comment to state the `CPU_SETSIZE` bound that makes `CPU_SET` sound. Extracting the mask-building loop into a pure `fn build_cpu_set(&[CpuId]) -> Result<libc::cpu_set_t, AffinityError>` would also make it unit-testable off-Linux (today the >=1024 path is only reachable on real big-iron hardware).

### 2. `parse_cpulist` expands ranges with no cap -- infallible public API aborts the process on large well-formed input
- **File:line:** `crates/shamir-numa/src/cpulist.rs:40` (`cpus.extend(lo..=hi)`); feed points `crates/shamir-numa/src/linux.rs:57` (`/sys/devices/system/node/online`) and `:76` (per-node cpulist)
- **Severity:** low
- **Issue:** The public parser (`pub fn parse_cpulist`, doc-advertised as "also useful standalone for tooling that inspects `/proc` / `/sys` cpu masks") is infallible yet expands every inclusive range unbounded: `"0-18446744073709551615"` (or even `"0-999999999"`) makes `extend(lo..=hi)` attempt an astronomical allocation -> capacity-overflow panic / OOM abort of the whole process. This contradicts the function's own documented contract ("malformed tokens ... skipped rather than erroring -- best-effort discovery should not abort"). All current callers feed kernel-generated sysfs text (trusted -- root-owned, masked in containers), so there is no exploit path today; the risk is any future untrusted source (config value, client-supplied string) turning this into a one-shot DoS. The same unbounded expansion applies to node ids parsed from the `online` file, which later size `NodeReplicated::new`'s per-node replica array (one cache-padded cell per node), amplifying the allocation.
- **Failure scenario:** Future tooling passes a user-supplied mask string (`"0-4294967295"`) -> `Vec` capacity overflow / OOM -> process abort at discovery time.
- **Suggested fix:** Bound the expansion to a sane ceiling (e.g. skip any range wider than 4096-8192, matching the sysfs `NR_CPUS` reality, or pre-count and skip over-long ranges) while keeping the infallible signature. Add an oversized-range test in `src/tests/cpulist_tests.rs` (the suite covers garbage/reversed/whitespace tokens but no range-size bound).

### 3. `probe` swallows all per-node cpulist read errors into an empty node -- later surfaces as a bare `EINVAL`
- **File:line:** `crates/shamir-numa/src/linux.rs:75-79` (`Err(_) => Vec::new()`), interacting with `:139-149`
- **Severity:** nit
- **Issue:** Every error from reading a node's cpulist file (permission denied under a hardened container, EIO, ...) is collapsed into "node exists but owns zero CPUs". `pin_current_thread_to_node` on such a node then hands `sched_setaffinity` an all-zero mask, which the kernel rejects with `EINVAL`; the caller sees `AffinityError::Syscall(EINVAL)` with no hint that discovery silently degraded. No memory-safety or privilege impact, but the fail-soft mapping destroys the diagnostic trail on the crate's only external input surface.
- **Failure scenario:** Container where `/sys/devices/system/node/node1/cpulist` is unreadable: topology reports node 1 with 0 CPUs; a later pin to node 1 fails with a misleading `EINVAL` instead of pointing at the unreadable sysfs file.
- **Suggested fix:** Map only `Err(NotFound)` to "node with an empty CPU list" (matching the documented container/NUMA-less case); propagate other error kinds (or at least keep them distinguishable) so a pin failure traces back to the unreadable file rather than a syscall misuse.

</details>

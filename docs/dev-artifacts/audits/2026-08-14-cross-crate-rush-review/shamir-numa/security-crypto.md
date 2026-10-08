<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-numa — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

No authentication or cryptographic boundary exists here. High-ID affinity panic is a conditional Linux API defect; parser exhaustion is an embedding-input risk rather than an established remote vulnerability.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 3 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `CPU_SET` fed sysfs CPU indices with no `CPU_SETSIZE` bound -- panic instead of `Err` on &gt;=1024-CPU hosts

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The exact libc 0.2.186 published linux_l4re_shared.rs:1531 indexes a fixed 1024-bit array. Sysfs CPU ID 1024 suffices for bounds panic when pinning is called. No C macro, memory corruption or production worker-pinning route is involved.

Evidence: [crates/shamir-numa/src/linux.rs:143](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L143); [Cargo.lock:1923](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1923); [crates/shamir-numa/tests/linux_topology.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/tests/linux_topology.rs#L19).

Grouping/duplicate: [error-handling-lifecycle.md#3](error-handling-lifecycle.md#review-3). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — `parse_cpulist` expands ranges with no cap -- infallible public API aborts the process on large well-formed input

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A syntactically valid large range expands without a budget. Hostile strings matter only if an embedding caller accepts them; current callers read kernel sysfs. Resource consumption is structural, while exact abort timing is unmeasured.

Evidence: [crates/shamir-numa/src/cpulist.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/cpulist.rs#L29); [crates/shamir-numa/src/cpulist.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/cpulist.rs#L40); [crates/shamir-numa/src/linux.rs:76](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L76).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — `probe` swallows all per-node cpulist read errors into an empty node -- later surfaces as a bare `EINVAL`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

A per-node permission or I/O error is discarded into an empty CPU list; a later pin cannot recover the original error. The empty mask has no eligible CPUs and can fail with EINVAL; no privilege escalation follows.

Evidence: [crates/shamir-numa/src/linux.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L75); [crates/shamir-numa/src/linux.rs:78](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L78); [crates/shamir-numa/src/linux.rs:145](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L145).

Grouping/duplicate: [error-handling-lifecycle.md#2](error-handling-lifecycle.md#review-2). This is not an additional independent defect.

## Evidence and recipe corrections

- The overview's unresolved-CPU_SET statement is obsolete relative to both its parent refinement and this independent exact-archive inspection.
- Cargo.toml:88 explicitly configures release panic=unwind. A CPU_SET panic is not automatically process abort; deployment boundaries still determine containment.
- The exact helper contains checked indexing, not the historical debug_assert/C-macro variants. Numeric high-ID input violates error handling, not Rust memory safety.
- A libc cpu_set_t builder remains Linux-specific. A portable mask-domain core must be separate from the Linux FFI adapter.
- Missing CAP_SYS_NICE alone is not a reason self-affinity fails; empty permitted intersections and platform restrictions are the relevant qualified cases. [Linux affinity documentation](https://man7.org/linux/man-pages/man2/sched_setaffinity.2.html).

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

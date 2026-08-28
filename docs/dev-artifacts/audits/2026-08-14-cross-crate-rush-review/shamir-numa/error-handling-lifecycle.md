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

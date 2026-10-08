<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-numa — style-claude-md revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Inline Linux tests and stale scope documentation remain. Inline placement is a low-severity convention issue, not missing registration; intentionally nonexecuted examples are not a structural violation.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 2 | 0 | 0 | 1 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Inline `#[cfg(test)] mod tests` in an implementation file

Status: `confirmed-open`. Current risk: `low`.

linux.rs still embeds the two tests contrary to the explicit test-layout rule. lib.rs includes that module on Linux, so the tests are reachable even though absent from the separate test manifest.

Evidence: [crates/shamir-numa/src/linux.rs:179](../../../../../crates/shamir-numa/src/linux.rs#L179); [crates/shamir-numa/src/lib.rs:47](../../../../../crates/shamir-numa/src/lib.rs#L47); [crates/shamir-numa/src/tests/mod.rs:8](../../../../../crates/shamir-numa/src/tests/mod.rs#L8); [CLAUDE.md:594](../../../../../CLAUDE.md#L594).

<a id="review-2"></a>

### Claim 2 — Stale "Фаза 1 scope" docs contradict shipped code

Status: `confirmed-open`. Current risk: `low`.

Scope/tier/roadmap prose still calls the Linux implementation forthcoming despite its exported probe and existing Linux tests. The QEMU smoke harness also already exists; only full guest Rust integration is pending.

Evidence: [crates/shamir-numa/src/lib.rs:34](../../../../../crates/shamir-numa/src/lib.rs#L34); [crates/shamir-numa/src/tests/mod.rs:5](../../../../../crates/shamir-numa/src/tests/mod.rs#L5); [crates/shamir-numa/README.md:79](../../../../../crates/shamir-numa/README.md#L79); [crates/shamir-numa/src/lib.rs:60](../../../../../crates/shamir-numa/src/lib.rs#L60); [scripts/ci-qemu-numa-test.sh:6](../../../../../scripts/ci-qemu-numa-test.sh#L6).

Grouping/duplicate: `api-wire-protocol.md#2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Doc example in `cpulist.rs` is never compiled or executed

Status: `refuted`. Current risk: —.

This is the explicitly sanctioned rendered-example policy, not a style defect. Behavioral parser tests are separately registered. Adding the exact illustrative literal to unit tests is optional strengthening, not a required repair.

Evidence: [crates/shamir-numa/Cargo.toml:10](../../../../../crates/shamir-numa/Cargo.toml#L10); [crates/shamir-numa/src/cpulist.rs:24](../../../../../crates/shamir-numa/src/cpulist.rs#L24); [crates/shamir-numa/src/tests/mod.rs:9](../../../../../crates/shamir-numa/src/tests/mod.rs#L9); [crates/shamir-numa/src/tests/cpulist_tests.rs:25](../../../../../crates/shamir-numa/src/tests/cpulist_tests.rs#L25); [crates/shamir-numa/src/tests/cpulist_tests.rs:36](../../../../../crates/shamir-numa/src/tests/cpulist_tests.rs#L36).

Grouping/duplicate: `correctness-tdd.md#7`. This row is not another independent defect.

## Corrections and qualified non-findings

- Confirmed structural non-findings: tests/mod.rs is manifest-only, lib.rs wires it under cfg(test), implementation imports are at file/module headers, and NodeId/CpuId form a closely coupled identifier pair.
- Downgrade original medium inline-test severity to low: placement alone does not cause runtime failure, and the tests are not dead.
- The report's only real-sysfs coverage wording is false: crates/shamir-numa/tests/linux_topology.rs also supplies registered Linux integration tests.
- Do not describe the QEMU Tier-3 smoke harness as future work. Full guest-side Rust test execution remains future work.
- No executable-doctest requirement should be introduced contrary to Cargo.toml's explicit policy.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-numa -- Style & CLAUDE.md structural conformance

## Summary

The crate is largely exemplary against CLAUDE.md's structural rules: `src/tests/mod.rs` is a manifest-only re-export file (the only `mod.rs` in the crate), wired through `lib.rs`'s `#[cfg(test)] mod tests;`; implementation lives in flat sibling files; all imports sit at file headers; and every file owns exactly one primary export (`node.rs`'s `NodeId`+`CpuId` is a legitimately closely-coupled identifier pair re-exported together, which the rule explicitly permits). One genuine violation exists: `src/linux.rs` embeds an inline `#[cfg(test)] mod tests` block instead of a file under `src/tests/`. Additionally, several "Фаза 1 scope" doc comments (`lib.rs`, `src/tests/mod.rs`, README) now contradict the shipped code, which already implements the "Фаза 1b" `LinuxTopology` work those comments describe as pending.

## Findings

### 1. Inline `#[cfg(test)] mod tests` in an implementation file

- **File:line:** `crates/shamir-numa/src/linux.rs:179-200`
- **Severity:** medium
- **Issue:** CLAUDE.md "Test organisation" rule 5 says: "Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory." `linux.rs` carries an inline two-test module (`probe_on_real_linux_host_succeeds`, `current_node_is_in_range`) behind `#[cfg(all(test, target_os = "linux"))]`, while the rest of the crate follows the `src/tests/<topic>_tests.rs` layout. There is no `src/tests/linux_tests.rs`, so the crate's test inventory is split across two layouts.
- **Failure scenario:** `src/tests/mod.rs`'s manifest does not list these tests, so a reader triaging `src/tests/` (or a future refactor that moves/renames `linux.rs`, or a cleanup that strips the inline block) can silently lose the crate's only real-sysfs `LinuxTopology` coverage.
- **Suggested fix:** Move the two tests to `src/tests/linux_tests.rs` and wire them from `src/tests/mod.rs` with `#[cfg(all(test, target_os = "linux"))] pub mod linux_tests;` (the `cfg` gate is needed because `src/tests/` compiles on every platform while `linux.rs` is Linux-only). Delete the inline module. The existing `use super::*;` inside the block is a documented import exception, but the block itself is not.

### 2. Stale "Фаза 1 scope" docs contradict shipped code

- **File:line:** `crates/shamir-numa/src/lib.rs:34-40`; `crates/shamir-numa/src/tests/mod.rs:1-6`; also `crates/shamir-numa/README.md:12,39-46,77-79`
- **Severity:** low
- **Issue:** `lib.rs`'s "# Scope of this version (Фаза 1)" section claims "Platform-independent skeleton only" and that "the real `LinuxTopology` (`/sys` probe + `sched_setaffinity`) ... land in Фаза 1b"; `src/tests/mod.rs` likewise says the real-`/sys` Tier-2 tests "land in Фаза 1b". But this version already ships `LinuxTopology::probe()` with `sched_setaffinity`/`sched_getcpu` (`src/linux.rs`), the Linux branch of `detect()` (`src/detect.rs:23-31`), the `libc` dependency (`Cargo.toml:25-30`, annotated "Фаза 1b"), the integration test `tests/linux_topology.rs`, and the inline Linux unit tests of finding 1.
- **Failure scenario:** A maintainer trusting `lib.rs`'s scope note assumes Linux/Tier-2 coverage does not exist yet and re-implements it, or mis-plans the next phase; because CLAUDE.md's discipline rules forbid touching unrelated comments piecemeal, these staleness spots otherwise never get corrected.
- **Suggested fix:** One small docs-only commit (per the "style/chore-only sweep" convention) updating `lib.rs`'s scope section, `src/tests/mod.rs`'s tier note, and the README roadmap to reflect that Фаза 1b's `LinuxTopology` and its tests have landed — leaving only the QEMU Tier-3 harness marked as future work.

### 3. Doc example in `cpulist.rs` is never compiled or executed

- **File:line:** `crates/shamir-numa/src/cpulist.rs:24-28`
- **Severity:** nit
- **Issue:** With `doctest = false` (`Cargo.toml:9-13`, per the project-wide doctest ban) the `parse_cpulist` example is pure illustration — the convention sanctions this — but it asserts specific behavior (`"0-1,4"` → `[CpuId(0), CpuId(1), CpuId(4)]`) that overlaps `src/tests/cpulist_tests.rs` coverage without any mechanism catching drift; the fenced code block is not even type-checked, let alone run.
- **Failure scenario:** If `parse_cpulist` semantics ever change, the rendered docs silently assert wrong behavior indefinitely while the unit tests (the real source of truth) may or may not be updated in step.
- **Suggested fix:** Keep the example, but either drop its assertions in favor of one illustrative call plus a pointer to `cpulist_tests.rs`, or ensure every case it demonstrates is also literally asserted in `cpulist_tests.rs` so the tests remain the single verified source of truth.

</details>

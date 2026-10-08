<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-numa — style-claude-md independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The inline Linux test block is a real layout-policy exception, and scope prose is stale. Test registration is intact; illustrative nonexecuted examples comply with manifest policy.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 2 | 0 | 0 | 1 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Inline `#[cfg(test)] mod tests` in an implementation file

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The implementation embeds two tests despite the explicit prohibition. lib.rs selects linux.rs under Linux cfg, so they are not dead. Move them into the existing manifest layout while preserving Linux-only compilation.

Evidence: [crates/shamir-numa/src/linux.rs:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/linux.rs#L179); [crates/shamir-numa/src/lib.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/lib.rs#L47); [AGENTS.md:146](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/AGENTS.md#L146); [CLAUDE.md:594](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L594).

<a id="review-2"></a>

### Claim 2 — Stale "Фаза 1 scope" docs contradict shipped code

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

LinuxTopology and Linux detect are implemented and exported, and the QEMU smoke script exists. Future-work prose remains in lib.rs, README and test-manifest documentation; full guest Rust integration is the actual pending portion.

Evidence: [crates/shamir-numa/src/lib.rs:34](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/lib.rs#L34); [crates/shamir-numa/src/lib.rs:60](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/lib.rs#L60); [crates/shamir-numa/src/tests/mod.rs:5](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/mod.rs#L5); [crates/shamir-numa/README.md:79](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/README.md#L79); [scripts/ci-qemu-numa-test.sh:6](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/scripts/ci-qemu-numa-test.sh#L6).

Grouping/duplicate: [api-wire-protocol.md#2](api-wire-protocol.md#review-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — Doc example in `cpulist.rs` is never compiled or executed

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Cargo.toml expressly keeps rendered examples nonexecuted, with behavioral tests registered separately. Exact-literal test coverage could be strengthened, but nonexecution is not a structural violation or a later fix.

Evidence: [crates/shamir-numa/Cargo.toml:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/Cargo.toml#L10); [crates/shamir-numa/src/tests/mod.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/mod.rs#L9); [crates/shamir-numa/src/tests/cpulist_tests.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-numa/src/tests/cpulist_tests.rs#L25).

Grouping/duplicate: [correctness-tdd.md#7](correctness-tdd.md#review-7). This is not an additional independent defect.

## Evidence and recipe corrections

- Inline placement is not missing registration, and integration tests also exercise the Linux factory/pin surface.
- Relocating tests must update imports formerly supplied by super::* and keep Linux-only items cfg-gated.
- The historical instruction to leave QEMU entirely marked future work is false; the shipped smoke oracle is narrower than full integration and needs the qualification identified below.
- No doctest-policy change or new commit is required by this read-only revalidation.

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

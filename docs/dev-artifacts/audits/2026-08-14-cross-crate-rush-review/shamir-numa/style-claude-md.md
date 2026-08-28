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

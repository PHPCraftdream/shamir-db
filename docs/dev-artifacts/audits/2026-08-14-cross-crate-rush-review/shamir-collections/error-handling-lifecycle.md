<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-collections — error-handling-lifecycle revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The constructors remain infallible and undocumented, with no local tests. The original only-abort wording and trusted-capacity call-site assurance are incorrect: capacity overflow can unwind, and a public decoder already forwards a declared MessagePack map count.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Infallible capacity constructors can only abort the process; no fallible counterpart exists

Status: `confirmed-open`. Current risk: `low`.

All four _wc helpers still allocate without returning reservation errors or documenting failure behavior. Pinned hashbrown distinguishes capacity-overflow panic from allocation-failure abort. The existing public MessagePack decoder forwards Map32's declared count to new_map_wc before reading entries; remote production reachability was not established.

Evidence: [crates/shamir-collections/src/lib.rs:29](../../../../../crates/shamir-collections/src/lib.rs#L29); [crates/shamir-collections/src/lib.rs:37](../../../../../crates/shamir-collections/src/lib.rs#L37); [crates/shamir-collections/src/lib.rs:53](../../../../../crates/shamir-collections/src/lib.rs#L53); [crates/shamir-collections/src/lib.rs:61](../../../../../crates/shamir-collections/src/lib.rs#L61); [Cargo.lock:1601](../../../../../Cargo.lock#L1601); [Cargo.toml:88](../../../../../Cargo.toml#L88); [crates/shamir-types/src/codecs/interned/messagepack.rs:251](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L251); [crates/shamir-types/src/codecs/interned/messagepack.rs:318](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L318).

<a id="review-2"></a>

### Claim 2 — Zero tests anywhere in the crate — exported contract has no regression net

Status: `confirmed-open`. Current risk: `low`.

There is still no local regression suite for builder identity, capacity or collection semantics. No Result-based cleanup path exists locally, but delegated capacity-overflow/allocation failure remains an observable boundary condition.

Evidence: [crates/shamir-collections/src/lib.rs:25](../../../../../crates/shamir-collections/src/lib.rs#L25); [crates/shamir-collections/src/lib.rs:63](../../../../../crates/shamir-collections/src/lib.rs#L63); [crates/shamir-collections/Cargo.toml:16](../../../../../crates/shamir-collections/Cargo.toml#L16).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

## Corrections and qualified non-findings

- Refute the literal only-abort assertion: pinned hashbrown panics on capacity overflow, and the workspace release profile uses panic=unwind. Ordinary allocation failure remains distinct.
- Refute the all-literals-or-materialized-lengths assurance: decode_map receives a Map16/Map32 header count, not a validated materialized collection length.
- Value's serde visitor separately clamps size_hint preallocation at value.rs:204 and value.rs:212; that mitigation does not cover the raw decode_map path.
- The backup manifest example is an existing caller, not hypothetical, although its requested capacity is the length of an already-deserialized vector.
- Documentation explains allocation policy but does not make hostile declared counts recoverable. Bound or fallibly reserve them at the decoder boundary when that API accepts untrusted bytes.
- Local absence of explicit unwrap/expect/panic/assert sites, error types, I/O, locks and custom Drop logic remains confirmed. Returned collections still own allocations, so no resources/no fallibility must not be interpreted as a transitive guarantee.
- Switching an IndexMap hasher alone does not change insertion-order iteration; the proposed failure explanation conflates hasher identity and ordered backing.
- No error cleanup or test success was verified by execution.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-collections -- Error handling & resource lifecycle

## Summary

The crate is a 63-line, dependency-light leaf (`src/lib.rs` + `Cargo.toml` only): four
public type aliases (`TMap`/`TSet`/`TFxMap`/`TFxSet`), `THasher`, and eight infallible
constructor functions. There is no fallible API surface — no `Result`, no error enum,
no I/O, no locks, no `Drop`-managed state — so most of this theme is vacuously clean.
Static scan found zero explicit panic sites (`unwrap`/`expect`/`panic!`/`assert`/
`todo!`). The two findings below are the honest residue: the constructors' infallible
allocation contract (panic-on-OOM) and the complete absence of any test coverage for
the exported alias/constructor contract.

## Findings

### 1. Infallible capacity constructors can only abort the process; no fallible counterpart exists

**File:** `crates/shamir-collections/src/lib.rs:29-31, 37-39, 53-55, 61-63`
(`new_map_wc`, `new_set_wc`, `new_fx_map_wc`, `new_fx_set_wc`)

**Severity:** low

**Issue:** All four `_wc` constructors take a caller-supplied `capacity: usize` and call
`IndexMap::with_capacity_and_hasher` / `HashMap::with_capacity_and_hasher`, which on
allocation failure (or integer overflow in the capacity computation) invoke
`handle_alloc_error` — i.e. **panic/abort**, not `Result`. CLAUDE.md's error-handling
rule ("Return `Result<T, E>`. Avoid `panic!` outside ... invariant violations") cannot be
satisfied by these fns under a hostile-capacity scenario. Mitigating context, verified by
workspace-wide grep of all ~100 call sites: every current capacity argument is a literal
(0–10) or `.len()` of an already-materialized in-memory collection
(`queries.len()`, `fields.len() + funcs.len()`, `manifest.files.len()`), so real OOM at
these sites implies the process was already over-committed and any allocation strategy
would be failing. No call site passes an untrusted/user-derived number directly.

**Failure scenario:** A future caller derives `capacity` from an untrusted bound (client
batch size hint, advertised manifest count, config knob) without pre-clamping it, e.g.
`new_fx_set_wc(manifest.files.len())` where `files.len()` comes from a parsed,
attacker-influenced backup manifest before validation → process abort instead of a
recoverable error surfaced to the operator.

**Suggested fix:** Either (a) document the infallible-allocation contract explicitly on
each `_wc` fn doc-comment ("panics via alloc failure, like `std`; pass clamped bounds"),
or (b) add `try_new_map_wc`/`try_new_fx_set_wc`-style variants returning
`Result<T, TryReserveError>` using indexmap's/the std fallback's fallible-reserve path,
and note in the docs which one is intended for untrusted-bound callers. Option (a) alone
is acceptable given the current call-site audit.

### 2. Zero tests anywhere in the crate — exported contract has no regression net

**File:** `crates/shamir-collections/src/lib.rs` (whole crate); `tests/` directory does not exist

**Severity:** low

**Issue:** Judged strictly against this theme: there are no error paths, therefore no
*error-path tests* are missing — nothing to report there. However, the brief also asks to
judge missing tests honestly: the crate ships zero tests of any kind while being the
workspace's foundational leaf (`THasher`, `TMap`, `TSet` are consumed by essentially
every other crate, and CLAUDE.md pillar #4 names them as normative). Nothing pins the
behavioral contract of the aliases: insertion-order preservation for `TMap`/`TSet`,
hasher identity (`THasher::default()` actually wired through, vs. accidentally switching
to `RandomState`), or dedup semantics of the set aliases. A silent drift here would
surface as nondeterministic iteration order bugs *in other crates*, far from the cause.
This is outside the pure "error-path" scope but falls under "missing tests" judged from
this lens; it is flagged at low severity rather than omitted because the fix is trivial
and the blast radius is workspace-wide.

**Failure scenario:** Someone edits `lib.rs` (e.g. swaps `IndexMap<K, V, THasher>` back
to `IndexMap<K, V>` default builder during an ill-advised cleanup). Lib gate
(`./scripts/test.sh`) stays green everywhere except consumers whose iteration-order
assumptions break — caught late, misattributed to engine/query logic.

**Suggested fix:** Add `src/tests/mod.rs` (per repo layout: re-export manifest +
topic files, wired via `#[cfg(test)] mod tests;`) with a handful of `#[test]`s:
insertion order preserved for `TMap`/`TSet`; `THasher` actually used
(e.g. constructing via `Default`/`new_map` yields the same type behavior);
`TFxSet` membership/dedup; `_wc` variants honor `capacity` ≥ len growth.
No async/runtime needed — pure value-level assertions, ~1 ms runtime.

## Explicit non-findings (checked, clean)

- **Result/thiserror discipline** — N/A: no function returns `Result`, none is fallible;
  nothing uses `anyhow`, `Box<dyn Error>`, or leaks errors across boundaries.
- **Panic avoidance** — no `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!`/
  `unimplemented!`/`assert*`/array indexing/slicing arithmetic anywhere in `src/lib.rs`
  (verified by regex scan of the whole file).
- **Error-path resource cleanup** — N/A: the crate acquires no resources (no files,
  sockets, locks, guard objects); every item is a type alias or a pure owned-value
  constructor with no partial-initialization window, so there is no cleanup path that
  could be skipped on an error route.
- **Workspace lint posture** — `#![allow(clippy::disallowed_types)]` is intentional and
  correctly scoped to this crate (it defines the sanctioned `std::HashMap`/`HashSet` +
  Fx escape-hatch aliases itself); it does not hide any panics or dropped results.

---

*Review basis: full read of `crates/shamir-collections/Cargo.toml` and
`crates/shamir-collections/src/lib.rs` (the entirety of the crate — confirmed via glob,
no submodules/tests/benches/examples exist), plus read-only grep of the ~100 workspace
call sites of the four `_wc` constructors. Read-only review; no code modified.*

</details>

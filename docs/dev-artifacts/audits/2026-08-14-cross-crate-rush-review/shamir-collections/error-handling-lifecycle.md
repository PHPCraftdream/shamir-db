<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-collections — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Constructors delegate ordinary collection allocation without a recoverable reservation API. Caller validation, rather than universal fallible leaf constructors, is the important boundary obligation.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Infallible capacity constructors can only abort the process; no fallible counterpart exists

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The corrected current claim remains: helpers are infallible and undocumented. Its literal only-abort title is false because hashbrown 0.17.1 panics on capacity overflow. A raw Map32 header reaches new_map_wc before entries are read, contradicting the historical trusted-capacity assurance. Missing try helpers alone is an API choice; accepting unchecked declared counts is the caller hazard. Source: https://docs.rs/crate/hashbrown/0.17.1/source/src/raw.rs.

Evidence: [crates/shamir-collections/src/lib.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L29); [crates/shamir-collections/src/lib.rs:53](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L53); [Cargo.lock:1600](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1600); [Cargo.toml:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.toml#L88); [crates/shamir-types/src/codecs/interned/messagepack.rs:251](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L251); [crates/shamir-types/src/codecs/interned/messagepack.rs:318](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L318).

<a id="review-2"></a>

### Claim 2 — Zero tests anywhere in the crate — exported contract has no regression net

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Local contract tests are absent; downstream coverage is real but incomplete. No local returned-error cleanup path exists, while delegated allocation failure remains observable. Capacity-overflow tests can distinguish panic versus returned error without attempting destructive OOM allocation.

Evidence: [crates/shamir-collections/src/lib.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L29); [crates/shamir-collections/src/lib.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L63); [crates/shamir-collections/Cargo.toml:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/Cargo.toml#L16).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

## Evidence and recipe corrections

- Ordinary owned collection constructors need not all return Result merely because allocation can fail. Document their delegated policy and validate hostile counts at accepting boundaries.
- The map and sequence Value visitors' 4096 initial-hint cap does not protect raw decode_map, raw decode_array, or RecordView::index.
- RecordView::index is another header-derived preallocation seam, not a validated materialized length; it uses TFxMap directly and would not be repaired by changing only new_* helpers.
- IndexMap fallible reservation reports its own TryReserveError; std collections use std::collections::TryReserveError. A shared try-helper recipe must preserve distinct error types or deliberately introduce a conversion.
- Reservation failure does not make subsequent key/value allocations or later growth universally fallible. Avoid claiming complete OOM recovery.
- The backup example exists at shamir-server/src/backup.rs:416, but its argument is an already-materialized manifest vector length.
- Default std-linked allocation failure aborts, whereas capacity overflow and configurable allocation handling are distinct. Reference: [Rust 1.94.0 allocation handler](https://github.com/rust-lang/rust/blob/1.94.0/library/alloc/src/alloc.rs).

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

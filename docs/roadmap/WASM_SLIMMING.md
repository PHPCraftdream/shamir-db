בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# WASM slimming — guest binary & SDK weight

**Status:** design / proposed (revision 2026-06-05, **post-investigation**).

Phased plan to shrink the **guest WASM** authors compile and the **guest
SDK** they depend on. Each phase is step-by-step, measured (before→after),
cheap wins before structural ones, correctness never depends on a size
knob. Companion: [`SDK_AUTHORING.md`](./SDK_AUTHORING.md),
[`FUNCTIONS.md`](./FUNCTIONS.md).

---

## Findings (from code investigation)

- **`shamir-sdk` uses `shamir-types` ONLY in tests** (`value_tests.rs`,
  `validation_tests.rs` — host↔guest conformance), never in prod guest
  code. → move it to `[dev-dependencies]` (not delete) — wasm stops
  pulling the heavy graph (`dashmap/parking_lot/arc-swap/rand/regex/
  chrono/rmpv/bincode/base-x/bytes`), conformance tests stay.
- **Guest `Value` mirror is LIGHTER than the host `Value<Key>`**: host has
  `Dec(Decimal)`/`Big(BigInt)`/`Set(IndexSet)` + `IndexMap`-Map (pulls
  `num-bigint`/`rust_decimal`/`indexmap`). The guest mirror is serde-only
  `Vec`-Map. **Unifying the type would GROW guest weight, not shrink it.**
  So: keep the lean mirror, guarantee no drift via a conformance test.
- Wire already lines up: host sends `Dec`/`Big` as **strings**, `Set` as a
  seq — the guest mirror reads them as `Str`/`List` (lossy, never panics).
  `value_tests.rs` already round-trips guest↔host.
- `wasm-opt`/binaryen is **not installed** locally; wasm32 target **is**.
- `Value<Key>` is generic; `Serialize`/`Deserialize` are generic (the
  String-key prefix-parsing is self-contained, no interner inside).

---

## Phase 0 — Measure baseline (no code change)
1. Add a tiny example guest crate (or use an existing `examples/`) with one
   `#[shamir::function]` identity fn; build:
   `cargo build --release --target wasm32-unknown-unknown -p <example>`.
2. Record the raw `.wasm` byte size (`wc -c`).
3. Install `cargo-bloat` if absent; run
   `cargo bloat --release --target wasm32-unknown-unknown -n 30` and
   `--crates`. Record the top weight contributors as text.
4. Commit nothing — write the numbers into this doc / the task as the
   yardstick for every later phase.

## Phase 1 — Move `shamir-types` to dev-dependencies (instant win, ~0 risk)
1. Grep-confirm `shamir-types` is unused in `crates/shamir-sdk/src/**`
   **outside** `tests/` (investigation says: only `tests/` use it).
2. In `crates/shamir-sdk/Cargo.toml`: remove `shamir-types` from
   `[dependencies]`, add it to `[dev-dependencies]`.
3. Fix the `value.rs` doc-comment so it doesn't imply a runtime dep
   ("wire-compatible with `QueryValue`", not "mirrors the dep").
4. Verify: `cargo build --release --target wasm32-unknown-unknown
   -p shamir-sdk` compiles; `cargo test -p shamir-sdk` (host) still green
   (conformance tests see the dev-dep).
5. Re-measure (Phase 0 yardstick) — confirm the heavy graph is gone.
6. Commit `chore(sdk): shamir-types → dev-dependency (guest no longer
   pulls the host graph)`.

## Phase 1b — Lock the wire contract with a conformance test (no drift)
1. Extend `crates/shamir-sdk/src/tests/value_tests.rs`: for EVERY shared
   shape — Null/Bool/Int/F64/Str/Bin/List/Map and nested — assert the guest
   `Value` ↔ host `QueryValue` **msgpack round-trip** is byte-identical
   (encode on one side, decode on the other, both directions).
2. Add the lossy-but-stable cases: host `Dec`/`Big` (strings) decode to
   guest `Str`; host `Set` (seq) decodes to guest `List` — assert exactly
   that, so the simplification is pinned, not accidental.
3. Run `cargo test -p shamir-sdk`. Green = the mirror can never silently
   drift from the host type.
4. Commit `test(sdk): host↔guest Value wire-conformance (pin the mirror)`.

## Phase 2 — `wasm-opt` in the compile pipeline (cheap; needs binaryen)
1. In `crates/shamir-engine/src/function/compile.rs`, after the cargo wasm
   build: if a `wasm-opt` binary is found (PATH check, like the cargo
   toolchain check), run `wasm-opt -Oz <in> -o <out>` and use the optimized
   artifact; **if absent → log a warn and use the unoptimized wasm** (never
   fail the compile).
2. Document: install binaryen (`choco install binaryen` / package / build)
   to enable it.
3. Measure with binaryen present (size↓) vs absent (skip, no regression).
4. Commit `perf(function): optional wasm-opt -Oz on compiled functions`.
> Blocked locally until binaryen is installed (user action). Until then,
> ship the graceful-skip wiring (step 1) and document.

## Phase 3 — Size profile for the scaffold build (cheap)
1. In `compile.rs`, the generated scaffold crate's `Cargo.toml`: add a
   `[profile.release]` with `opt-level = "z"` (or `"s"`), `lto = true`,
   `codegen-units = 1`, `panic = "abort"`, `strip = true`.
2. Scope strictly to the **scaffold** crate — never touch the server's own
   `[profile.*]`.
3. Re-measure (`panic="abort"` also drops unwinding tables).
4. Commit `perf(function): size profile for the wasm scaffold build`.

## Phase 4 — (optional, NOT for weight) lean `shamir-value` ABI crate
> Do this only if **crate-graph cleanliness / a single `Value` type** is
> worth it — it does **not** reduce guest weight (the full `Value<Key>`
> pulls bigint/decimal/indexmap). Must be feature-gated so the guest stays
> lean. Phase 1b already removes the drift risk more cheaply.
1. Gate the serde-genericness check: confirm `Serialize`/`Deserialize` of
   `Value<Key>` carry no host-only logic beyond the self-contained
   String-prefix parsing (investigation: confirmed).
2. New crate `crates/shamir-value/`: move `Value<Key>` + serde + `PartialEq`/
   `Eq`/`Hash`/`to_bytes`/`from_bytes` (+ `TMap`/`TSet` or depend on a
   tiny common). Feature-gate the heavy variants: `#[cfg(feature =
   "extended")] Dec(Decimal)` / `Big(BigInt)` / `Set(...)`; default build
   = no extended (guest stays light, reads them as Str/List on the wire).
3. `shamir-types`: `QueryValue`/`InnerValue`/`UserValue` re-export
   `shamir_value::Value<…>`; enable `extended`; keep interner/codecs/access
   host-side. `InternerKey` stays in `shamir-types`, parameterizes the ABI
   type.
4. `shamir-sdk`: guest `Value = shamir_value::Value<String>` (no
   `extended`) — delete the hand-written mirror.
5. **Wire byte-identity is mandatory:** Phase-1b conformance tests must
   stay green across the move. Do it in its own commit, measured.

## Recommended order
```
P0 measure → P1 dev-deps → P1b conformance → P3 size-profile      ← cheap, safe, the real wins
           → P2 wasm-opt (when binaryen is installed)
           → P4 shamir-value  ← optional, crate-cleanliness only, feature-gated, NOT for weight
```

## Metrics
- Raw `.wasm` bytes (scalar example; later a procedure example).
- `cargo bloat --target wasm32-unknown-unknown --release` top crates/fns.
- Per-feature size matrix once SDK feature-gates land (see SDK_AUTHORING).

## Guardrails
- Correctness never depends on a size knob; `wasm-opt` absent → skip+log.
- Size profile scoped to the scaffold crate only.
- Wire format byte-identical across any `Value` change (P1b/P4 tests).
- "Don't over-build": stop when the scalar wasm is small; P4 only on real
  need for crate cleanliness, never for weight.

## Open / user actions
- Install **binaryen** to enable Phase 2 locally (user).
- `query-types` is heavy (pulls `shamir-types` + hmac/sha2/zeroize) — its
  guest-slimming for builder-in-guest is tracked in
  [`SDK_AUTHORING.md`](./SDK_AUTHORING.md).

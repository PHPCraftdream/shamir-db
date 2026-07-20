# F4 — SQ8 SIMD kernels for `approx_l2_sq`/`fused_dot`/`dequant_norm_sq` (+ optional query-norm hoist)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context — READ THE WHOLE MODULE DOC FIRST

Fourth item of "Этап 8 — Performance", sourced from report 07
(`docs/dev-artifacts/research/2026-07-17-release-audit/
07-performance-optimizations.md`), finding **F4** (severity High for vector
workloads) — read that report's F4 section (lines 136-161) first.

**Then read `crates/shamir-index/src/vector/quantized_dist.rs` lines 1-124
in full — its module doc IS the authoritative VR-7 (#429) architecture
analysis** this task is scoped against. It already:
- proves Cosine is ~3.5× slower than L2 (measured, `benches/sq8_hot_path.rs`),
  entirely due to two `dequant_norm_sq` passes per `eval` call;
- exhaustively considered FOUR fix options for a query-norm cache and
  explicitly **rejected/deferred three of them** (per-internal-id cache
  can't reach `eval`; pointer-keyed cache needs an eviction policy + a
  pointer-stability guard **not scoped here**; folding the norm into the
  wire format is a much larger refactor);
- says option 2 (query-norm hoist) "could land as a follow-up" but flags a
  REAL correctness hazard: `ShamirDistU8::eval` is symmetric/ID-blind
  (`hnsw_rs`'s `Distance<T>::eval(&self, a, b)` never tells you which side,
  if either, is "the query"), the distance object is shared and cloned onto
  rayon thread-locals, and a per-search stash "needs a stack of
  (query_ptr → norm) entries to be safe" under reentrancy.

**This task's mandatory scope is deliberately narrower than "implement all
of VR-7"** — see "What's MANDATORY vs OPTIONAL" below. Do not attempt the
full query-norm-hoist-for-`eval`'s-graph-traversal unless you can build and
prove it safe; the module's own analysis already tells you this is hard.

## The key insight that shrinks this task (verify it yourself before coding)

`Sq8Quantizer` (`crates/shamir-index/src/vector/sq8.rs`) already precomputes,
at fit time: `scales_sq: Vec<f32>` (= `s_i²`, line 61), `min_scale: Vec<f32>`
(= `min_i·s_i`, line 65), and `min_sq_sum: f32` (= `Σ min_i²`, line 68).
`approx_dot` (lines 222-255) already routes through the existing
`weighted_bilinear_f32` SIMD kernel (`vector/simd.rs`, task #614), computing
`min_sq_sum + Σ min_scale[i]·(qx[i]+qy[i]) + Σ scales_sq[i]·qx[i]·qy[i]`.

**Verify this identity yourself** (algebra, not code-reading): calling
`weighted_bilinear_f32(min_scale, scales_sq, q, q)` (the SAME code slice
passed as BOTH `qx` and `qy`) computes
`Σ min_scale[i]·(q[i]+q[i]) + scales_sq[i]·q[i]·q[i] = Σ 2·min_scale[i]·q[i] +
scales_sq[i]·q[i]²`. Adding `min_sq_sum` gives
`Σ min_i² + 2·min_i·s_i·q[i] + s_i²·q[i]² = Σ (min_i + q[i]·s_i)²` — **exactly
`dequant_norm_sq`'s definition**, currently duplicated (and NOT SIMD-ised) in
TWO places: `ShamirDistU8::dequant_norm_sq` (lines 173-185) and
`RescoreCtx::dequant_norm_sq` (lines 386-397). **This means `dequant_norm_sq`
needs ZERO new SIMD kernel code — it can reuse the EXISTING, already-tested
`weighted_bilinear_f32` kernel with `q` passed twice.** This is the
lowest-risk, highest-value part of this task — do it first.

## What's MANDATORY vs OPTIONAL

### MANDATORY

1. **Canonicalize `dequant_norm_sq` on `Sq8Quantizer` itself, SIMD-backed
   via the existing kernel (zero new unsafe code).** Add:
   ```rust
   /// `‖dequant(q)‖² = Σ (min_i + q_i·s_i)²`, computed via the existing
   /// `weighted_bilinear_f32` SIMD kernel with `q` passed as BOTH operands
   /// (see this module's/this task's derivation: `min_sq_sum +
   /// weighted_bilinear_f32(min_scale, scales_sq, q, q)` expands to exactly
   /// this sum — no new kernel needed).
   pub fn dequant_norm_sq(&self, q: &[u8]) -> f32 {
       self.min_sq_sum + crate::vector::simd::weighted_bilinear_f32(&self.min_scale, &self.scales_sq, q, q)
   }
   ```
   in `sq8.rs`, near `approx_dot`/`approx_l2_sq`. Then:
   - `ShamirDistU8::dequant_norm_sq` (`quantized_dist.rs:173-185`): replace
     the scalar loop body with `self.params.dequant_norm_sq(q)` (or delete
     the method entirely and update its ONE call site in `eval`'s Cosine arm,
     `quantized_dist.rs:219-220`, to call `self.params.dequant_norm_sq(a)` /
     `self.params.dequant_norm_sq(b)` directly — your choice, whichever
     reads cleaner; either way, delete the old duplicated scalar loop).
   - `RescoreCtx::dequant_norm_sq` (`quantized_dist.rs:386-397`): same —
     replace with a call to `self.params.dequant_norm_sq(codes)` (note
     `RescoreCtx::params: &'a Sq8Quantizer`, confirm the field name/type at
     `quantized_dist.rs:271` before wiring), or delete the method and update
     its call site (`quantized_dist.rs:361`) directly.
   - **Verify the panics/assertions are preserved**: both old methods had a
     `debug_assert_eq!(q.len(), self.params.dim())`-style length check (one
     via `debug_assert_eq!` at line 174, one implicitly via the caller's own
     `assert_eq!` in `RescoreCtx::score`). Decide whether `Sq8Quantizer::
     dequant_norm_sq` should carry its own `debug_assert_eq!` (recommended,
     matches `approx_dot`/`approx_l2_sq`'s own assert style) so the new
     canonical method is self-defending regardless of caller.
2. **New SIMD kernel for `approx_l2_sq`** (`sq8.rs:295-325`) — this one
   genuinely has NO existing kernel to reuse (it's a squared-DIFFERENCE, not
   a bilinear product). Add `weighted_sq_diff_u8(scales_sq: &[f32], qx: &[u8],
   qy: &[u8]) -> f32` to `vector/simd.rs`, computing
   `Σ scales_sq[i]·(qx[i] − qy[i])²`. **Mirror `weighted_bilinear_f32`'s
   EXACT file structure and naming convention** (read `simd.rs` lines
   765-963 in full as your template): a scalar reference
   (`weighted_sq_diff_scalar`), a dispatcher (`weighted_sq_diff_u8`,
   AVX2 → NEON → scalar, matching `has_avx2()`/`has_neon()`'s existing
   cached-detection helpers), an AVX2 `#[target_feature(enable =
   "avx2,fma")]` kernel, and a NEON `#[target_feature(enable = "neon")]`
   kernel, each with a scalar tail loop for the remainder past the last full
   SIMD chunk. The AVX2 widening (`_mm_loadl_epi64` → `_mm256_cvtepu8_epi32`
   → `_mm256_cvtepi32_ps`) and NEON widening
   (`vld1_lane_u32`/`vmovl_u8`/`vmovl_u16`/`vcvtq_f32_u32`) are IDENTICAL to
   `weighted_bilinear_avx2`/`weighted_bilinear_neon`'s own widening code —
   copy that exact sequence, just replace the "linear + bilinear" FMA math
   with "subtract, then square, then scale":
   `diff = xv - yv; acc = fmadd(ssv, diff*diff, acc)` (AVX2:
   `_mm256_fmadd_ps(ssv, _mm256_mul_ps(diff, diff), acc)` where
   `diff = _mm256_sub_ps(xv, yv)`; NEON: `vfmaq_f32(acc, ssv, vmulq_f32(diff,
   diff))` where `diff = vsubq_f32(xv, yv)`). Route `Sq8Quantizer::
   approx_l2_sq` (`sq8.rs:295-325`) through it, replacing the scalar loop
   with a single call: `crate::vector::simd::weighted_sq_diff_u8(&self.scales_sq,
   qx, qy)`.
3. **New SIMD kernel for `RescoreCtx::fused_dot`** (`quantized_dist.rs:
   372-381`) — currently `acc = self.qm; for i in 0..codes.len() { acc +=
   self.qs[i] * (codes[i] as f32) }`, a single f32-weight × u8-code linear
   sum with NO bilinear/subtract term (simpler than both kernels above). Add
   `weighted_linear_u8(weights: &[f32], codes: &[u8]) -> f32` to `simd.rs`
   computing `Σ weights[i]·codes[i]`, same dispatcher/scalar/AVX2/NEON
   structure (the AVX2/NEON widening is again identical to
   `weighted_bilinear_*`'s; the accumulate step is a single FMA per lane
   instead of two). Route `fused_dot` through it:
   `self.qm + crate::vector::simd::weighted_linear_u8(&self.qs, codes)`.

### OPTIONAL (attempt ONLY if you can build and PROVE it safe — do not force this)

4. **VR-7 option 2 — query-norm hoist for `ShamirDistU8::eval`'s Cosine arm**
   (the GRAPH TRAVERSAL path — `hnsw_u8.search(...)`'s internal per-hop
   `eval` calls, NOT the rescore path, which already hoists the query norm
   via `RescoreCtx::q_norm`). This is the ONE piece of VR-7 the module doc
   flags as genuinely hard: `eval(a, b)` is symmetric and doesn't know which
   side (if either) is "the query" being searched for in the CURRENT
   `.search()` call, and `ShamirDistU8` is shared across concurrent/nested
   searches on rayon thread-locals.

   If you attempt this, the mechanism must be: a `thread_local!` cell (e.g.
   `RefCell<Vec<(usize, f32)>>`, a STACK keyed by the query slice's pointer
   address, pushed in `search_quantized_graph`/`search_cofilter_quantized`
   right before calling `hnsw_u8.search(...)`/`.search_filter(...)` and
   popped right after — matching the module doc's own suggested shape) so
   that:
   - it is correct under a single sequential `.search()` call (the common
     case — verify whether `hnsw_rs`'s `.search()` for ONE query is
     single-threaded; if you cannot confirm this from `hnsw_rs`'s own docs/
     source, treat it as a hard blocker and do NOT proceed with this item);
   - it degrades to "no hit, recompute the norm" (NOT a panic, NOT a stale
     value) if the stack is empty or the top entry's pointer doesn't match
     either `a` or `b` — a soft-miss, exactly like this campaign's other
     opt-in caches (`CondCache`/`FieldPathCache`/`QueryRefCache` — the
     PREVIOUS three tasks in this Этап 8 batch, if you want a style
     reference for the "graceful degradation on a cache miss" pattern,
     though the mechanism itself here is a thread-local, not a
     `FilterContext`-threaded map);
   - you add tests PROVING correctness under: (a) a normal single search,
     (b) two sequential searches on the same thread (stack must not leak
     entries between calls), (c) if you can construct it, a scenario with
     nested/concurrent searches on the same adapter (even a
     `tokio::join!`/thread-spawn-based test) proving no cross-contamination.

   **If at any point you are not confident this is correct** (e.g. you
   cannot verify `hnsw_rs`'s single-query threading model, or the stack
   discipline feels fragile), **STOP, revert this item only (keep items
   1-3), and explain in your summary exactly what you found and why you
   stopped** — citing the module doc's own precedent for treating this as
   a deferrable follow-up is a COMPLETELY ACCEPTABLE outcome for this task,
   not a failure to deliver. Do not ship a plausible-looking but unverified
   concurrency mechanism just to "complete" this optional item.

## Verification (MANDATORY before you report done, covers items 1-3 always; item 4 only if attempted)

- **New kernel tests**, mirroring the EXACT pattern already used for
  `weighted_bilinear_f32` in `crates/shamir-index/src/vector/tests/
  simd_tests.rs` (read lines 154-270 as your template — dispatcher-equals-
  scalar across a dim sweep, a multi-seed random sweep, a zero-dim edge
  case, an all-255 edge case, and a "the AVX2 path is actually exercised on
  this host" sanity check using the same `has_avx2()` gate). Add this FULL
  suite of test shapes for BOTH new kernels (`weighted_sq_diff_u8` and
  `weighted_linear_u8`) — do not skip the zero-dim/all-255/non-multiple-of-
  lane-width edge cases, they are exactly what would catch an off-by-one in
  a hand-written SIMD tail loop.
- **`Sq8Quantizer::dequant_norm_sq` equivalence test**: prove the NEW
  canonical method produces IDENTICAL output (bit-for-bit or within the
  existing test tolerance — check what `assert_close` in `simd_tests.rs`
  uses) to the OLD scalar formula (`Σ(mins[i]+q[i]*scales[i])²`) computed
  independently in the test itself (not by calling the code you just
  deleted — write the reference formula fresh in the test), across a
  dim/seed sweep.
- **`Sq8Quantizer::approx_l2_sq` / `RescoreCtx::fused_dot` unaffected-output
  tests**: if `crates/shamir-index/src/vector/tests/quantized_dist_tests.rs`
  (or similar — locate the actual file) already has tests pinning these
  functions' output, confirm they STILL PASS UNCHANGED (this is a pure perf
  fix, zero behavior change for correct inputs). If no such test exists,
  add one narrow test per function proving the SIMD-routed result matches
  an independently-computed scalar reference.
- `./scripts/test.sh -p shamir-index --full` green — run it TWICE (this
  session's flake-triage discipline). Pay close attention to ANY vector
  search correctness/recall test — a mistake in the new SIMD kernels would
  most likely show up as a wrong ranking or a NaN, not a crash.
- Re-run `crates/shamir-index/benches/sq8_hot_path.rs`'s
  `shamir_dist_u8_eval/Cosine/128` cell (`CARGO_TARGET_DIR=D:/dev/rust/
  .cargo-target-bench cargo bench -p shamir-index --bench sq8_hot_path`,
  forward slashes only) and report the before/after numbers — the module
  doc's own baseline is ~243 µs; report what you measure after items 1-3
  land, honestly (a modest, not necessarily 3.5×, improvement is expected
  since the norm computation is only PART of the Cosine cost — the module
  doc itself says a full fix needs item 4, which you may not have
  attempted).
- `cargo fmt -p shamir-index -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace — SIMD intrinsic code is exactly where clippy catches real
  mistakes, do not `#[allow]` your way past a genuine warning).
- Report literal command output for all of the above, and explicitly state
  whether you attempted item 4 (optional) and its outcome (implemented +
  proven safe, OR explicitly deferred with your reasoning).

## Out of scope

- Do NOT touch F5 (ForEach, task 8e) or F6/F9/F10/F11 (task 8f).
- Do NOT touch anything from tasks 8a/8b/8c (already landed, different
  crate/module) or Этапы 1-7.
- Do NOT implement VR-7 options 1, 3, or 4 (per-internal-id cache,
  pointer-keyed cache with eviction, wire-format norm folding) — the module
  doc already rejected/deferred these for reasons unrelated to this task's
  scope; only option 2 is even discussed here, and only as optional item 4.
- Do NOT change `_sync`/`_async` method choices, deadlock-fix comments, or
  anything in `hnsw_adapter.rs` beyond what item 4 (if attempted) strictly
  requires for the thread-local push/pop around the two `.search`/
  `.search_filter` call sites.

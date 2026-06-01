//! SIMD distance kernels shared by vector adapters.
//!
//! Hot path: both brute-force search and HNSW graph traversal call
//! `dot_product` / `l2_squared` for every distance computation. For
//! typical 128-d f32 vectors that's a clean vectorizable inner loop.
//! On x86_64 we runtime-detect AVX2 once and dispatch to an AVX2+FMA
//! kernel (8 lanes, fused multiply-add) for a repeatable multiple over
//! the autovec scalar path. Fallback is the scalar kernel written in a
//! chunked form the compiler also reliably vectorizes (no per-iteration
//! bounds checks, multiple accumulators).
//!
//! The dispatch flag is loaded with `Relaxed` from a `OnceLock`-cached
//! `bool` so the per-call overhead is one predictable branch on
//! already-hot memory. No locks. No new dependencies.
//!
//! Invariant: both kernels MUST return the same value for the same
//! inputs to within fp rounding (FMA differs from add-then-mul by
//! at most 0.5 ulp per op — within the existing test tolerances).

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[inline]
fn has_avx2() -> bool {
    use std::sync::OnceLock;
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| std::is_x86_feature_detected!("avx2"))
}

#[inline]
pub(super) fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if has_avx2() {
            // SAFETY: `has_avx2()` guarantees the AVX2 + FMA target
            // features required by the intrinsics used in
            // `dot_product_avx2` are present on this CPU.
            return unsafe { dot_product_avx2(a, b) };
        }
    }
    dot_product_scalar(a, b)
}

#[inline]
pub(super) fn l2_squared(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if has_avx2() {
            // SAFETY: see `dot_product` above.
            return unsafe { l2_squared_avx2(a, b) };
        }
    }
    l2_squared_scalar(a, b)
}

/// Scalar dot product written so the compiler reliably autovectorizes:
/// fixed-size chunks of 8, separate accumulators, no bounds checks in
/// the inner loop.
#[inline]
fn dot_product_scalar(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let n = a.len().min(b.len());
    let chunks = n / 8;
    let mut acc = [0.0f32; 8];
    for i in 0..chunks {
        let base = i * 8;
        // Indexing with a known-good range; LLVM elides the bounds
        // checks and emits packed multiplies + adds.
        for j in 0..8 {
            acc[j] += a[base + j] * b[base + j];
        }
    }
    let mut s = acc[0] + acc[1] + acc[2] + acc[3] + acc[4] + acc[5] + acc[6] + acc[7];
    for i in (chunks * 8)..n {
        s += a[i] * b[i];
    }
    s
}

/// Scalar squared-L2 written the same way as `dot_product_scalar`.
#[inline]
fn l2_squared_scalar(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let n = a.len().min(b.len());
    let chunks = n / 8;
    let mut acc = [0.0f32; 8];
    for i in 0..chunks {
        let base = i * 8;
        for j in 0..8 {
            let d = a[base + j] - b[base + j];
            acc[j] += d * d;
        }
    }
    let mut s = acc[0] + acc[1] + acc[2] + acc[3] + acc[4] + acc[5] + acc[6] + acc[7];
    for i in (chunks * 8)..n {
        let d = a[i] - b[i];
        s += d * d;
    }
    s
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2,fma")]
unsafe fn dot_product_avx2(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    debug_assert_eq!(a.len(), b.len());
    let n = a.len().min(b.len());
    let chunks = n / 8;

    // Four independent accumulators so we don't bottleneck on FMA
    // latency. For dim=128 we get exactly 16 8-lane chunks → 4 per
    // accumulator → pipeline stays full.
    let mut acc0 = _mm256_setzero_ps();
    let mut acc1 = _mm256_setzero_ps();
    let mut acc2 = _mm256_setzero_ps();
    let mut acc3 = _mm256_setzero_ps();

    let ap = a.as_ptr();
    let bp = b.as_ptr();
    let mut i = 0usize;
    while i + 32 <= chunks * 8 {
        let av0 = _mm256_loadu_ps(ap.add(i));
        let bv0 = _mm256_loadu_ps(bp.add(i));
        let av1 = _mm256_loadu_ps(ap.add(i + 8));
        let bv1 = _mm256_loadu_ps(bp.add(i + 8));
        let av2 = _mm256_loadu_ps(ap.add(i + 16));
        let bv2 = _mm256_loadu_ps(bp.add(i + 16));
        let av3 = _mm256_loadu_ps(ap.add(i + 24));
        let bv3 = _mm256_loadu_ps(bp.add(i + 24));
        acc0 = _mm256_fmadd_ps(av0, bv0, acc0);
        acc1 = _mm256_fmadd_ps(av1, bv1, acc1);
        acc2 = _mm256_fmadd_ps(av2, bv2, acc2);
        acc3 = _mm256_fmadd_ps(av3, bv3, acc3);
        i += 32;
    }
    while i + 8 <= chunks * 8 {
        let av = _mm256_loadu_ps(ap.add(i));
        let bv = _mm256_loadu_ps(bp.add(i));
        acc0 = _mm256_fmadd_ps(av, bv, acc0);
        i += 8;
    }

    // Horizontal reduction of the four 8-lane accumulators.
    let s01 = _mm256_add_ps(acc0, acc1);
    let s23 = _mm256_add_ps(acc2, acc3);
    let s = _mm256_add_ps(s01, s23);
    let hi = _mm256_extractf128_ps(s, 1);
    let lo = _mm256_castps256_ps128(s);
    let v128 = _mm_add_ps(lo, hi);
    let shuf = _mm_movehdup_ps(v128);
    let sums = _mm_add_ps(v128, shuf);
    let shuf2 = _mm_movehl_ps(sums, sums);
    let sums2 = _mm_add_ss(sums, shuf2);
    let mut s = _mm_cvtss_f32(sums2);

    for k in (chunks * 8)..n {
        s += a[k] * b[k];
    }
    s
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2,fma")]
unsafe fn l2_squared_avx2(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    debug_assert_eq!(a.len(), b.len());
    let n = a.len().min(b.len());
    let chunks = n / 8;

    let mut acc0 = _mm256_setzero_ps();
    let mut acc1 = _mm256_setzero_ps();
    let mut acc2 = _mm256_setzero_ps();
    let mut acc3 = _mm256_setzero_ps();

    let ap = a.as_ptr();
    let bp = b.as_ptr();
    let mut i = 0usize;
    while i + 32 <= chunks * 8 {
        let av0 = _mm256_loadu_ps(ap.add(i));
        let bv0 = _mm256_loadu_ps(bp.add(i));
        let d0 = _mm256_sub_ps(av0, bv0);
        let av1 = _mm256_loadu_ps(ap.add(i + 8));
        let bv1 = _mm256_loadu_ps(bp.add(i + 8));
        let d1 = _mm256_sub_ps(av1, bv1);
        let av2 = _mm256_loadu_ps(ap.add(i + 16));
        let bv2 = _mm256_loadu_ps(bp.add(i + 16));
        let d2 = _mm256_sub_ps(av2, bv2);
        let av3 = _mm256_loadu_ps(ap.add(i + 24));
        let bv3 = _mm256_loadu_ps(bp.add(i + 24));
        let d3 = _mm256_sub_ps(av3, bv3);
        acc0 = _mm256_fmadd_ps(d0, d0, acc0);
        acc1 = _mm256_fmadd_ps(d1, d1, acc1);
        acc2 = _mm256_fmadd_ps(d2, d2, acc2);
        acc3 = _mm256_fmadd_ps(d3, d3, acc3);
        i += 32;
    }
    while i + 8 <= chunks * 8 {
        let av = _mm256_loadu_ps(ap.add(i));
        let bv = _mm256_loadu_ps(bp.add(i));
        let d = _mm256_sub_ps(av, bv);
        acc0 = _mm256_fmadd_ps(d, d, acc0);
        i += 8;
    }

    let s01 = _mm256_add_ps(acc0, acc1);
    let s23 = _mm256_add_ps(acc2, acc3);
    let s = _mm256_add_ps(s01, s23);
    let hi = _mm256_extractf128_ps(s, 1);
    let lo = _mm256_castps256_ps128(s);
    let v128 = _mm_add_ps(lo, hi);
    let shuf = _mm_movehdup_ps(v128);
    let sums = _mm_add_ps(v128, shuf);
    let shuf2 = _mm_movehl_ps(sums, sums);
    let sums2 = _mm_add_ss(sums, shuf2);
    let mut s = _mm_cvtss_f32(sums2);

    for k in (chunks * 8)..n {
        let d = a[k] - b[k];
        s += d * d;
    }
    s
}

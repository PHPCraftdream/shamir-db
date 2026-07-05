//! Bit-exact equality tests for the integer u8 dot-product kernel
//! (`dot_u8`) against the scalar reference (`dot_u8_scalar`).
//!
//! DoD requirement: the SIMD dispatch path MUST equal the scalar
//! reference BIT-FOR-BIT (integer `==`, no float tolerance) on arbitrary
//! u8 inputs INCLUDING the saturation edge (all-255, dim ≥ 32, where a
//! naïve `maddubs`-based kernel would clip and diverge).
//!
//! On x86_64 dev/CI hosts the dispatcher selects the AVX2 kernel, so this
//! suite exercises that kernel end-to-end. The NEON `udot`/widening paths
//! are type-checked via `cargo check --target aarch64-*` and mirror the
//! proven AVX2/scalar structure (see the block comment in `simd.rs`).

use crate::vector::simd::{dot_u8, dot_u8_scalar};

/// Numerical-Recipes LCG (same lineage as `shamir_bench_utils::Lcg` and
/// `hnsw_rs_contract_tests::lcg_vec`) — deterministic, no global RNG.
struct Lcg {
    state: u64,
}
impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    #[inline]
    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }
    #[inline]
    fn next_u8(&mut self) -> u8 {
        (self.next_u64() >> 33) as u8
    }
}

fn rand_vec(dim: usize, seed: u64) -> Vec<u8> {
    let mut rng = Lcg::new(seed);
    (0..dim).map(|_| rng.next_u8()).collect()
}

const DIMS: &[usize] = &[1, 7, 8, 15, 16, 31, 32, 64, 127, 128];

#[test]
fn dispatcher_equals_scalar_random() {
    for &dim in DIMS {
        let a = rand_vec(dim, dim as u64 * 7 + 1);
        let b = rand_vec(dim, dim as u64 * 31 + 11);
        let want = dot_u8_scalar(&a, &b);
        let got = dot_u8(&a, &b);
        assert_eq!(
            got, want,
            "dispatcher != scalar on random dim={dim}: got {got}, want {want}"
        );
    }
}

#[test]
fn dispatcher_equals_scalar_all_255_saturation_edge() {
    // The critical saturation test: every product is 255*255 = 65025.
    // For dim=128 the sum is 8 323 200 — well within u32, but a naïve
    // `maddubs_epi16` kernel saturates EACH i16 pair to 32 767 and would
    // report a wildly different number. This is the guard against that.
    for &dim in &[32, 64, 127, 128, 256] {
        let a = vec![255u8; dim];
        let b = vec![255u8; dim];
        let want = dot_u8_scalar(&a, &b);
        let got = dot_u8(&a, &b);
        assert_eq!(
            got,
            want,
            "all-255 saturation dim={dim}: got {got}, want {want} (expected {})",
            65025 * dim
        );
        assert_eq!(want, (65025 * dim) as u32, "scalar sanity for dim={dim}");
    }
}

#[test]
fn dispatcher_equals_scalar_all_zero() {
    for &dim in DIMS {
        let a = vec![0u8; dim];
        let b = vec![0u8; dim];
        assert_eq!(dot_u8(&a, &b), 0, "all-zero dim={dim}");
        assert_eq!(dot_u8_scalar(&a, &b), 0, "all-zero scalar dim={dim}");
    }
}

#[test]
fn dispatcher_equals_scalar_mixed_high_low() {
    // Mix of >127 (which `maddubs` would reinterpret as negative i8) and
    // low codes — catches the signedness trap on the second operand.
    for &dim in &[32, 64, 128] {
        let mut a = vec![0u8; dim];
        let mut b = vec![0u8; dim];
        for i in 0..dim {
            a[i] = if i % 2 == 0 { 255 } else { 200 };
            b[i] = if i % 3 == 0 { 255 } else { 128 };
        }
        let want = dot_u8_scalar(&a, &b);
        let got = dot_u8(&a, &b);
        assert_eq!(
            got, want,
            "mixed high/low dim={dim}: got {got}, want {want}"
        );
    }
}

#[test]
fn dispatcher_equals_scalar_self_dot() {
    // x · x with x all-255: every term 65025, sum grows with dim. Another
    // saturation-sensitive configuration.
    for &dim in &[33, 64, 100, 128] {
        let a = vec![255u8; dim];
        let want = dot_u8_scalar(&a, &a);
        let got = dot_u8(&a, &a);
        assert_eq!(got, want, "self-dot all-255 dim={dim}");
    }
}

#[test]
fn dispatcher_equals_scalar_many_seeds() {
    // Statistical sweep: 200 random pairs at dim=128.
    for s in 0..200u64 {
        let a = rand_vec(128, s.wrapping_mul(2654435761).wrapping_add(1));
        let b = rand_vec(128, s.wrapping_mul(40503).wrapping_add(7));
        let want = dot_u8_scalar(&a, &b);
        let got = dot_u8(&a, &b);
        assert_eq!(got, want, "seed sweep s={s}: got {got}, want {want}");
    }
}

#[test]
fn scalar_reference_value_known() {
    // Pin a concrete known value so a regression in the scalar reference
    // itself is caught (not just relative agreement).
    // a = [0,1,...,15], b = [15,14,...,0]. Σ i*(15-i) for i=0..16
    // = 15*Σi - Σi² = 15*120 - 1240 = 1800 - 1240 = 560.
    let a: Vec<u8> = (0..16u8).collect();
    let b: Vec<u8> = (0..16u8).rev().collect();
    assert_eq!(dot_u8_scalar(&a, &b), 560);
    assert_eq!(dot_u8(&a, &b), 560);
}

// NOTE: `dot_u8` requires equal-length inputs (the SQ8 quantizer always
// passes same-dimension code vectors). There is intentionally NO test for
// mismatched lengths — that contract is shared with `dot_product` and is
// enforced by the `debug_assert_eq!` in each kernel. A release build
// truncates to the shorter length, matching `dot_product`'s behaviour.

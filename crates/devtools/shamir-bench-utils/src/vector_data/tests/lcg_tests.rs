use crate::vector_data::{InvalidRangeBounds, Lcg, LCG_INC, LCG_MULT};

fn seed_for_successor(successor: u64) -> u64 {
    successor
        .wrapping_sub(LCG_INC)
        .wrapping_mul(0xc097_ef87_329e_28a5)
}

#[test]
fn next_f32_maps_predecessor_seeds_to_exact_24_bit_endpoints() {
    for (successor, expected) in [
        (0, 0.0_f32),
        (0x0000_00ff_ffff_ffff, 0.0),
        (0x0000_0100_0000_0000, 1.0 / 16_777_216.0),
        (0xc000_0000_0000_0000, 0.75),
        (u64::MAX, 1.0 - 1.0 / 16_777_216.0),
        (0xffff_ff80_0000_0000, 1.0 - 1.0 / 16_777_216.0),
    ] {
        let seed = seed_for_successor(successor);
        assert_eq!(seed.wrapping_mul(LCG_MULT).wrapping_add(LCG_INC), successor);
        let mut rng = Lcg::new(seed);
        assert_eq!(rng.next_f32().to_bits(), expected.to_bits());
        let mut reference = Lcg::new(successor);
        assert_eq!(rng.next_u64(), reference.next_u64());
    }
}

#[test]
fn next_range_clamps_signed_adjacent_and_subnormal_endpoints() {
    let tiny = f32::from_bits(1);
    let bounds = [
        (1.0, f32::from_bits(1.0_f32.to_bits() + 1)),
        (f32::from_bits((-1.0_f32).to_bits() + 1), -1.0),
        (-tiny, -0.0),
        (0.0, tiny),
        (-2.0 * tiny, -tiny),
        (tiny, 2.0 * tiny),
    ];
    for (lo, hi) in bounds {
        for successor in [0, 0xc000_0000_0000_0000, u64::MAX] {
            let mut rng = Lcg::new(seed_for_successor(successor));
            let value = rng.try_next_range(lo, hi).unwrap();
            assert!(value >= lo && value < hi, "{lo:?} <= {value:?} < {hi:?}");
            assert_eq!(value, lo);
            let mut reference = Lcg::new(successor);
            assert_eq!(rng.next_u64(), reference.next_u64());
        }
    }
}

#[test]
fn next_range_interpolates_finite_extremes_and_consumes_one_step() {
    for (successor, expected) in [
        (0, -f32::MAX),
        (0x8000_0000_0000_0000, 0.0),
        (0xc000_0000_0000_0000, f32::MAX / 2.0),
    ] {
        let seed = seed_for_successor(successor);
        let mut checked = Lcg::new(seed);
        let mut legacy = Lcg::new(seed);
        let result = checked.try_next_range(-f32::MAX, f32::MAX).unwrap();
        assert_eq!(result.to_bits(), expected.to_bits());
        assert_eq!(
            legacy.next_range(-f32::MAX, f32::MAX).to_bits(),
            result.to_bits()
        );
        let mut reference = Lcg::new(successor);
        let next = reference.next_u64();
        assert_eq!(checked.next_u64(), next);
        assert_eq!(legacy.next_u64(), next);
    }
    let mut rng = Lcg::new(seed_for_successor(u64::MAX));
    let value = rng.next_range(-f32::MAX, f32::MAX);
    assert!(value.is_finite() && (-f32::MAX..f32::MAX).contains(&value));
}

#[test]
fn invalid_range_bounds_error_and_nan_leave_state_untouched() {
    let cases = [
        (1.0, 1.0),
        (1.0, -1.0),
        (-0.0, 0.0),
        (f32::NAN, 1.0),
        (0.0, f32::NAN),
        (f32::NEG_INFINITY, 1.0),
        (0.0, f32::INFINITY),
        (f32::INFINITY, f32::NEG_INFINITY),
    ];
    for (lo, hi) in cases {
        let mut checked = Lcg::new(42);
        let mut legacy = Lcg::new(42);
        let mut reference = Lcg::new(42);
        assert_eq!(checked.try_next_range(lo, hi), Err(InvalidRangeBounds));
        assert!(legacy.next_range(lo, hi).is_nan());
        let next = reference.next_u64();
        assert_eq!(checked.next_u64(), next);
        assert_eq!(legacy.next_u64(), next);
    }
}

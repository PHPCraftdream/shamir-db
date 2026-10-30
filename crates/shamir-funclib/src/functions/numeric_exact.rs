//! Exact numeric representation shared by ordering and distinct hashing.
//!
//! Finite values are reduced as numerator / (2^twos * 5^fives).
//! Removing common factors makes equal values hash identically across variants.

use num_bigint::BigInt;
use shamir_types::types::value::QueryValue;
use std::cmp::Ordering;
use std::hash::Hash;

#[derive(Hash)]
pub(super) enum ExactNumber {
    NegativeInfinity,
    Finite {
        numerator: BigInt,
        twos: u32,
        fives: u32,
    },
    PositiveInfinity,
    NaN,
}

impl ExactNumber {
    pub(super) fn from_value(value: &QueryValue) -> Self {
        match value {
            QueryValue::Int(n) => Self::finite(BigInt::from(*n), 0, 0),
            QueryValue::Big(n) => Self::finite(n.clone(), 0, 0),
            QueryValue::Dec(n) => Self::finite(BigInt::from(n.mantissa()), n.scale(), n.scale()),
            QueryValue::F64(n) if n.is_nan() => Self::NaN,
            QueryValue::F64(n) if *n == f64::NEG_INFINITY => Self::NegativeInfinity,
            QueryValue::F64(n) if *n == f64::INFINITY => Self::PositiveInfinity,
            QueryValue::F64(n) => {
                let bits = n.to_bits();
                let exponent = ((bits >> 52) & 0x7ff) as i32;
                let fraction = bits & ((1_u64 << 52) - 1);
                let significand = if exponent == 0 {
                    fraction
                } else {
                    fraction | (1_u64 << 52)
                };
                let power = if exponent == 0 {
                    -1074
                } else {
                    exponent - 1075
                };
                let mut numerator = BigInt::from(significand);
                if bits >> 63 != 0 {
                    numerator = -numerator;
                }
                if power >= 0 {
                    Self::finite(numerator << power as usize, 0, 0)
                } else {
                    Self::finite(numerator, (-power) as u32, 0)
                }
            }
            _ => unreachable!("numeric rank required"),
        }
    }

    fn finite(mut numerator: BigInt, mut twos: u32, mut fives: u32) -> Self {
        if numerator == BigInt::from(0) {
            return Self::Finite {
                numerator,
                twos: 0,
                fives: 0,
            };
        }
        while twos > 0 && &numerator % 2_u8 == BigInt::from(0) {
            numerator /= 2_u8;
            twos -= 1;
        }
        while fives > 0 && &numerator % 5_u8 == BigInt::from(0) {
            numerator /= 5_u8;
            fives -= 1;
        }
        Self::Finite {
            numerator,
            twos,
            fives,
        }
    }

    pub(super) fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (
                Self::Finite {
                    numerator: a,
                    twos: at,
                    fives: af,
                },
                Self::Finite {
                    numerator: b,
                    twos: bt,
                    fives: bf,
                },
            ) => {
                let left = (a << *bt as usize) * BigInt::from(5_u8).pow(*bf);
                let right = (b << *at as usize) * BigInt::from(5_u8).pow(*af);
                left.cmp(&right)
            }
            _ => self.rank().cmp(&other.rank()),
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Self::NegativeInfinity => 0,
            Self::Finite { .. } => 1,
            Self::PositiveInfinity => 2,
            Self::NaN => 3,
        }
    }
}

use super::{FailureState, LockoutState, PairKey, Subnet};

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct PairSlot {
    pub(super) key: PairKey,
    pub(super) failure: FailureState,
    pub(super) lockout: Option<LockoutState>,
    pub(super) occupied: bool,
    // Explicit reserved bytes keep every charged slot exactly 128 bytes.
    _padding: [u8; 48],
}

impl PairSlot {
    pub(super) fn new(key: PairKey, failure: FailureState, lockout: Option<LockoutState>) -> Self {
        Self {
            key,
            failure,
            lockout,
            occupied: true,
            ..Self::default()
        }
    }
}

impl Default for PairSlot {
    fn default() -> Self {
        Self {
            key: (Subnet::V4([0; 3]), [0; 16]),
            failure: FailureState {
                count: 0,
                last_fail_at_ns: 0,
            },
            lockout: None,
            occupied: false,
            _padding: [0; 48],
        }
    }
}

const _: [(); 128] = [(); core::mem::size_of::<PairSlot>()];

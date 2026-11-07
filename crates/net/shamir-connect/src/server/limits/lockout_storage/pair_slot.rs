use super::{FailureState, LockoutState, PairKey, Subnet};

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct PairSlot {
    pub(super) key: PairKey,
    pub(super) failure: FailureState,
    pub(super) lockout: Option<LockoutState>,
    pub(super) occupied: bool,
    _index: u16,
    _padding: [u8; 48],
}

impl PairSlot {
    pub(super) fn set_index(&mut self, index: u16) {
        self._index = index;
    }

    pub(super) fn index(&self) -> u16 {
        self._index
    }

    pub(super) fn new(key: PairKey, failure: FailureState, lockout: Option<LockoutState>) -> Self {
        Self {
            key,
            failure,
            lockout,
            occupied: true,
            ..Self::default()
        }
    }

    pub(super) fn clear_payload(&mut self) {
        let index = self._index;
        *self = Self::default();
        self._index = index;
    }

    pub(super) fn replace_payload(
        &mut self,
        key: PairKey,
        failure: FailureState,
        lockout: Option<LockoutState>,
    ) {
        let index = self._index;
        *self = Self::new(key, failure, lockout);
        self._index = index;
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
            _index: u16::MAX,
            _padding: [0; 48],
        }
    }
}

const _: [(); 128] = [(); core::mem::size_of::<PairSlot>()];

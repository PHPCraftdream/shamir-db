//! Layer b (high) with an inline module.

pub mod inner {
    pub fn go() -> u32 {
        1
    }
}

pub struct Engine;

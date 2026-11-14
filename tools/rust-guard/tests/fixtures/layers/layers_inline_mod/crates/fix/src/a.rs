//! Layer a (low) reaching into an inline module.

pub struct Thing;

pub fn probe() -> u32 {
    crate::b::inner::go()
}

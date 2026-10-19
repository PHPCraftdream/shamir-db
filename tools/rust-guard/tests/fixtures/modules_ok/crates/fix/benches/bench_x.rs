// Bench target that mod-includes the SHARED helper below.
mod common;

pub fn x() -> u32 {
    common::c()
}

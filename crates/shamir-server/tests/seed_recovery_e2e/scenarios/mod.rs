//! The four certification scenarios (design PR-5 row, section 6 of the
//! binding review). All are `#[tokio::test]` functions here; see each
//! module's docs for what is proven and what is explicitly NOT claimed.

pub mod a_crash_reseed;
pub mod b_second_crash;
pub mod c_refusals;
pub mod d_restarts;

mod range;
pub(crate) mod receipt;
#[cfg(test)]
pub(crate) mod test_seam;

pub use range::in_flight_range;

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;

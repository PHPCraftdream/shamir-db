// A brand-new file that introduces crate-visible items is itself a
// widening of the crate's surface (finding 10c).
pub(crate) fn added() -> u32 {
    1
}

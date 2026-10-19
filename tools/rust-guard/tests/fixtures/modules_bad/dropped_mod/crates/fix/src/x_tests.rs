// A test file whose `mod x_tests;` declaration was dropped: it still
// compiles, but the guard must reject the tree.
pub fn lost_test() -> u32 {
    2
}

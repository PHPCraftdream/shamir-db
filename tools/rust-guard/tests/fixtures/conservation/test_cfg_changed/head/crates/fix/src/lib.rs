pub struct Plan {
    pub a: u32,
}

pub fn helper(x: u32) -> u32 {
    x + 1
}

// The test gains a cfg predicate between the revisions: the item pairs
// by identity, but its cfg text (None -> `feature = "verbose"`) drifts.
#[cfg(feature = "verbose")]
#[test]
fn t_cfg() {
    assert_eq!(1 + 1, 2);
}

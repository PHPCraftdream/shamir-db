// Real (comment-only) head diff so the two commits differ; the test
// asserts BASE_MISMATCH from the stale delta pin.
pub struct Plan {
    pub a: u32,
}

impl Plan {
    pub fn run(&self) -> u32 {
        self.a
    }
}

pub fn helper(x: u32) -> u32 {
    x + 1
}

#[test]
fn t_ok() {
    assert_eq!(1 + 1, 2);
}

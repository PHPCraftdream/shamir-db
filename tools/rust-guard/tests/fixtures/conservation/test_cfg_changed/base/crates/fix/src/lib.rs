pub struct Plan {
    pub a: u32,
}

pub fn helper(x: u32) -> u32 {
    x + 1
}

#[test]
fn t_cfg() {
    assert_eq!(1 + 1, 2);
}

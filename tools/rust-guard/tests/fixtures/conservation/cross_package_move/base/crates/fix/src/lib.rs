pub fn stays(x: u32) -> u32 {
    x + 1
}

pub fn carried(x: u32) -> u32 {
    x * 2
}

#[test]
fn carried_test() {
    assert_eq!(carried(2), 4);
}

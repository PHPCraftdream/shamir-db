pub struct Plan {
    pub a: u32,
}

impl Plan {
    pub fn run(&self) -> u32 {
        self.a
    }
}

#[test]
fn helper2() {
    assert_eq!(1 + 1, 2);
}

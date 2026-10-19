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

// One base item (`helper`), two head candidates with the same
// kind/name/enclosing and signature but different module paths — no
// way to choose a pairing: UNPAIRABLE_DUPLICATE.
mod m1 {
    pub fn helper(x: u32) -> u32 {
        x + 1
    }
}

mod m2 {
    pub fn helper(x: u32) -> u32 {
        x + 1
    }
}

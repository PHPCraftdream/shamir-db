// Two head candidates for the base `dup` (same kind/name/enclosing and
// signature but different module paths): the pairing is ambiguous, so
// `dup` is UNPAIRABLE_DUPLICATE and never suggested.
mod d1 {
    pub fn dup() -> u32 {
        9
    }
}

mod d2 {
    pub fn dup() -> u32 {
        9
    }
}

mod m;

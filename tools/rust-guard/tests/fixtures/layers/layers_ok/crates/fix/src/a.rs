//! Layer a (low): no upward references.

pub struct Thing;

impl Thing {
    pub fn new() -> Thing {
        Thing
    }
    pub fn value(&self) -> usize {
        1
    }
}

pub fn helper() -> usize {
    2
}

#[cfg(test)]
mod tests {
    // Test code never fails the gate, even when it reaches upward.
    use crate::b::Engine;

    #[test]
    fn reaches_upward() {
        let _ = Engine;
    }
}

//! Layer b (high): may refer down to a.

use crate::a::{helper, Thing};

pub struct Engine;

impl Engine {
    pub fn run(&self) -> usize {
        Thing::new().value() + helper()
    }
}

pub struct S;

impl S {
    // A function-local `use` INSIDE an impl method body — the old
    // traversal only looked at top-level fn bodies (finding 10e).
    pub fn f(&self) -> String {
        use std::fmt;
        fmt::format(format_args!("x"))
    }
}

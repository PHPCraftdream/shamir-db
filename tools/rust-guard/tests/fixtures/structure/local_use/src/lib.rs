pub fn f() -> u32 {
    // A function-local `use` outside the documented exceptions:
    use std::fmt;
    fmt::format(format_args!("x"));
    1
}

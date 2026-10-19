// Path-mounted test module: the #[path] NAME-VALUE attribute must be
// read by the shared extraction (parse_args::<LitStr>() cannot).
#[path = "tests_alias/t.rs"]
mod mounted_tests;

pub fn existing() -> u32 {
    1
}

#[path = "twin.rs"]
mod a;

#[path = "twin.rs"]
mod b;

pub fn call() -> u32 {
    a::f() + b::f()
}

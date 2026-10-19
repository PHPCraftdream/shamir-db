#[cfg(unix)]
#[path = "shared.rs"]
mod a;
#[cfg(not(unix))]
#[path = "shared.rs"]
mod b;
